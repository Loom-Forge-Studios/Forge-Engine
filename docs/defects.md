# Defects — the one allocator (W8)

This file is the **only** place a defect number is allocated. Grep before allocating.
`cargo xtask allocators` (part of `just verify`) fails if an id appears twice or out of order.

Rules:

- Ids are `D-NNNN`, allocated **append-only**: the next id is one more than the last row.
- An id is an identity from the moment anything cites it (a commit, a test, a gate row).
  Never renumber, never reuse, never delete a row — a fixed defect is marked `fixed`, a
  non-defect `rejected`, with the reason.
- A Windows/Linux behavioural difference is a defect, never a caveat (I18).
- Each row names the guard that now catches it. A defect with no guard can recur silently.

| Id | Status | Summary | Found by | Guard that now catches it |
|---|---|---|---|---|
| D-0001 | fixed | serde_json parsed floats without correct rounding, so a command log re-read from a store could carry a value 1 ulp off (`262682861718.86282` came back as `262682861718.8628`) and replaying a project's history built a different project. Fixed by the workspace `float_roundtrip` feature | `forge spine` reload check (WP-07) | `crates/forge-store/tests/test_command_log_exact.rs` (gate row C-command-log-exact); `forge spine`'s `reload-log` self-check |
