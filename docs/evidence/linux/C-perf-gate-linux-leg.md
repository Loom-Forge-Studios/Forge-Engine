# C-perf-gate-linux-leg — the perf gate on lavapipe (observed; still AWAITING a baseline)

Image `forge-linux-verify:1.98.1` (`sha256:c960b17d10838574eb866789250ce2e81c409649b57c216d36d900bbb058e222`) from `rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e`; environment in [README](README.md); 2026-09-23.

```
perf gate measurements (llvmpipe (LLVM 15.0.6, 256 bits) (Vulkan 1.3)), device class None, calibration 21.341 ms
not gated: render.frame.gpu.shadow.cascade0: 6.857 ms (no baseline or slack for device class None)
not gated: render.frame.gpu.shadow.cascade1: 7.850 ms
not gated: render.frame.gpu.shadow.cascade2: 9.332 ms
not gated: render.frame.gpu.shadow.cascade3: 11.628 ms
not gated: render.frame.gpu.shell.sky: 15.638 ms
not gated: render.frame.gpu.shell.mid: 9.191 ms
not gated: render.frame.gpu.shell.near: 16.298 ms
not gated: render.frame.gpu.tonemap: 2.926 ms
not gated: render.frame.gpu.total: 79.719 ms
not gated: render.atmosphere.precompute: 19.848 ms
test result: ok. 8 passed; 0 failed
```

The CPU-ratio and counter rows (calibrated against the machine's own reference loop) are
gated on Linux and green; the GPU pass rows print NOT GATED (lavapipe has no committed
baseline) and the two GPU-row controls report NOT APPLICABLE for the same reason.

**Why the row stays AWAITING:** a lavapipe GPU baseline must come from the machine that
will gate it — CI's ubuntu runner — not from a Docker VM that shares the dev box's CPUs with
two Windows lanes (ADR 0044: budgets are neither gated nor tuned for the container). The
figures above are recorded as an observation only.
