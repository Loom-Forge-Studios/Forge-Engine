# C-replay-linux-leg (the committed play session replays on ubuntu-x86_64)

Image `forge-linux-verify:1.98.1` (`sha256:c960b17d10838574eb866789250ce2e81c409649b57c216d36d900bbb058e222`) from `rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e`; environment in [README](README.md); 2026-09-23. Part of the green full run in [verify-linux-run](verify-linux-run.md); the excerpts below are from `just verify-linux bash -c "…"` on the same snapshot (tree `01dcffd7`).

**Command:** `cargo test -p forge-sim --test test_replay --locked` (Linux):

```
test a_crash_truncated_recording_replays_up_to_where_it_ends ... ok
test another_scene_is_refused_and_a_one_ulp_input_change_is_caught_at_its_step ... ok
test the_committed_golden_session_replays_on_this_build ... ok
test positive_control_a_frame_rate_dependent_step_fails_replay ... ok
test a_recorded_session_replays_bit_for_bit ... ok
test result: ok. 5 passed; 0 failed
```

`the_committed_golden_session_replays_on_this_build` replays
`crates/forge-sim/tests/data/golden_session.forgereplay` (recorded on Windows; sha256
`a56ab43ba9b8e5295212b3b1d2365d1dc44d0ea43988b8081497244dad8f5d5f` in both checkouts) with its
per-step checks, and requires the Linux build to *record* the same file byte for byte today.
The determinism corpus of the same run is in [C-determinism-linux-leg](C-determinism-linux-leg.md).
