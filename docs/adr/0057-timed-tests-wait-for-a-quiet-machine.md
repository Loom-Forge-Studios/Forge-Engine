# ADR 0057 — Timed tests wait for a quiet machine; every wall-clock gate is checked to time alone

- **Status:** accepted
- **Date:** 2026-09-26
- **Plan references:** Ch.29 (budgets, the perf gate), §21.22 (UI budgets, D-5), W1/W2/W5,
  ADR 0028 (amendment: the conditions the perf gate measures under), ADR 0042 (the timed-alone
  lock and the GPU turn), ADR 0046 (`control_switches!`). WP-40; supersedes backlog L-28.

## Context

WP-19 made every wall-clock gate time its body through
`forge_trace::timed::run_timed_alone`: a child process running only that test, at the High
priority class, holding a machine-wide reader-writer lock that keeps timed bodies apart from
each other and from GPU tests. That lock cannot see **another worktree's compiler**. High
priority wins the CPU scheduler's queue, but a dozen `rustc`/`link.exe` processes still take
memory bandwidth, cache, turbo headroom and SMT siblings.

W5 forbids widening a budget or retrying. The fix must be scheduling.

## Decision

1. **Wait for a quiet machine, inside the lock** (`forge_trace::timed::quiet`). After the
   parent takes the exclusive timed lock (so no other timed body starts and new GPU turns
   queue behind it) it samples the share of all cores busy **outside its own process** over
   ~1 s and, while that is above **35 %**, sleeps 3 s and samples again. The wait is bounded
   at **10 minutes** (`FORGE_TIMED_QUIET_MAX_S` overrides; `0` = sample once, never wait).
   When the bound expires the body runs anyway and is reported **MEASURED UNDER LOAD**; the
   result is still asserted (W5), and a failure carries the report in its message.
2. **After an expiry, shorter waits.** An expired bound means the machine is persistently
   busy (the other lane's full run, a game). For 15 minutes afterwards the bound is 60 s, so
   twenty timed bodies do not wait ten minutes each. Faked waits (controls) never record one.
3. **Sampling without `unsafe`.** forge-trace stays `#![forbid(unsafe_code)]` (the plan names
   the crates allowed `unsafe`; forge-trace is not one). Linux reads `/proc/stat` and
   `/proc/self/stat`. Windows asks a PowerShell helper — the route `set_priority_class` already
   takes — that declares `GetSystemTimes` once and answers one line of counters per request,
   so a wait pays one start-up (~1.1 s measured) however many samples it takes. Cost per timed
   body on a quiet machine: ~2 s (start-up + one 1 s sample); on Linux ~1 s.
4. **Observable.** Every wait prints one line with the test's output (waited, samples, first /
   peak / last load, threshold, bound, outcome), appends it to `forge-timed-alone.log` in the
   temp directory (kept under 1 MiB) and bumps per-process counters (`quiet_counters`); the
   last report is readable per thread (`last_quiet_report`).
5. **Every wall-clock gate is checked** (`cargo xtask timed-gates`, in `just verify` and CI
   after `gpu-turn`): a test file that reads `Instant::now` either times through the helper
   or declares `// timed-gates: exempt(<reason>)` (a hang guard, a figure printed for the
   record); a file that times through the helper is a test binary named in the nextest
   `wall-clock` override (`max-threads = 1`, `threads-required = "num-cpus"`); every name in
   that override is a real test binary.

## Consequences

- Timed gates measure on a quiet machine whenever one appears within the bound; a lane's
  timed body now waits for the other lane's build to finish instead of failing beside it.
  Both decision rules: the user (and the integration loop) stops chasing load flakes, and the
  budgets keep their teeth — nothing was widened.
- A full test run pays ~2 s per timed body on Windows when the machine is already quiet
  (~25 bodies: under a minute), and waits as long as another lane keeps the cores busy, up to
  the bound.
- Threshold choice: the idle desktop measured 14-25 % outside the test process (Docker
  Desktop's backend and the Task Manager among it); 35 % leaves headroom above that while
  one `rustc` codegen burst (several cores) is still seen as busy.
- The check is per file and textual: a file that times through the helper is trusted for all
  its tests, and unit-test modules under `src/` are not scanned (their clock reads are hang
  guards today). A gate added elsewhere than a `tests/` file is the reviewer's to route.

## Positive controls (W2)

- `crates/forge-trace/tests/test_timed_quiet.rs::positive_control_a_busy_machine_is_waited_for_then_reported`
  — `QuietFaults::fake_load` (a `control_switches!` set; forge-trace gains a `controls`
  feature, dev-dependencies only) fakes 90 % load: the gate re-samples, waits out a 2 s bound,
  runs the body and reports MEASURED UNDER LOAD. Breaking the wait (treating every sample as
  quiet) fails it and `a_failure_under_load_says_so`.
