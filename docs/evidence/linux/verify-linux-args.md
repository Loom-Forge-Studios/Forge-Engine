# `just verify-linux` argument passing (backlog L-16)

Environment in [README](README.md); image `forge-linux-verify:1.98.1`; 2026-09-24.

Before the fix, an argument containing a space or a leading dash could be split or dropped
across three independent hops between `just verify-linux <args>` and the container:

1. `powershell -File`'s parameter binder prefix-matched any leading-dash argument (e.g. `-c`)
   against the script's declared `-Command` parameter name instead of collecting it as a
   remaining argument, failing with `Missing an argument for parameter 'Command'`.
2. `just`'s `{{ARGS}}` joins the recipe's variadic arguments with plain spaces and does not
   re-quote one that itself contains a space, so the grouping of a multi-word argument is lost
   unless the caller supplies the quoting itself.
3. Windows PowerShell 5.1 splatting an array to a native command (`docker @Run`) did not
   reliably preserve an embedded space or `"` character within one array element, splitting it
   into several `docker` arguments.

Fixed in `tools/docker/linux-verify/verify-linux.ps1`: reads `$args` (no declared parameter
name to collide with), and builds the final `docker` invocation as one hand-quoted command line
run through `cmd.exe` (`Invoke-DockerLine`) instead of an array splat.

**Commands and results** (each run with `--rm`, discarded after):

| Command | What it proves | Result |
|---|---|---|
| `just verify-linux bash -c '"echo INTACT a b"'` | a caller-quoted, space-containing argument reaches the container as one argument, unsplit | printed `INTACT a b` (one line); exit 0 |
| `just verify-linux bash -c '"exit 7"'` | the container's exit code still propagates through the fixed call path | `just` reported "failed ... with exit code 7" |
| `just verify-linux cargo --version` | plain multi-word arguments (no embedded spaces) are unaffected | printed `cargo 1.98.1 (797e8a9bc 2026-08-05)`; exit 0 |

The first row is the one this backlog item asked for: a spaced/quoted argument reaches the
container intact. The outer single quotes above are the calling shell's; the inner double
quotes are literal characters that must reach `just` for the argument to survive the
`just -> cmd.exe -> powershell -File` hop as one argument — documented in the script's own
header alongside this file.
