# ADR 0027 — The sky: Bruneton tables from composition on the GPU, and a perf gate with device-class baselines

- **Status:** accepted; decision 7 (GPU band) amended by ADR 0028
- **Date:** 2026-09-21
- **Plan references:** Ch.10 §10.5 (amended), Ch.29 (implementation section), DoD M1-8,
  M1-11, the M1 exit criterion; owner rules 1 and 2

## Context

WP-11 closes M1: an atmosphere parameterised by its composition (M1-8) and named perf
budgets with a CI gate (M1-11). The plan fixes "Bruneton-style precomputed multiple
scattering" and "a perf gate that fails CI on regression"; it leaves open where the tables
are computed, how composition becomes coefficients, and how a timing gate can hold on
machines of different speeds.

## Decision

1. **`forge-sky` (f64, deterministic)** holds the sky's physics: `AtmosphereBody` (gases as
   mole fractions, pressure, temperature, gravity, aerosols, ozone) derives Bruneton's
   coefficients from first principles (Rayleigh cross-sections from each gas's refractivity
   and King factor — the standard N2/O2 column reproduces Bucholtz's 0.0973 at 550 nm to
   3 % — scale height from mean molar mass, Mie from optical depth and Angstrom exponent,
   ozone from its column), plus an `f64` reference integrator. Every number has a physical
   derivation and a test.

2. **The tables are computed on the GPU** by compute shaders (transmittance, direct
   irradiance, single scattering, then per order density / indirect irradiance / multiple
   scattering), per unit solar irradiance, in ~40 ms on the RTX 3080 at Bruneton's resolution
   (rule 2: a change of air rebuilds the sky in a few frames, not at load time only). The
   renderer's own sky functions are evaluated through a probe and held against the `f64`
   reference (worst 1.7 %).

3. **Three deliberate departures from Bruneton 2017, each found by a guard:**
   (a) *single Mie scattering has its own table* (all channels) instead of being extrapolated
   from the red channel in the scattering table's alpha — the extrapolation was 18 % wrong at a
   low sun under wavelength-dependent (continental) haze; one more 3D table (~4 MB) and two
   samples per lookup buy an accurate sky (rule 1);
   (b) *the transmittance table stores optical depth*, not its exponential — grazing blue
   transmittance is subnormal in `Rgba16Float`, and the ratio of two such values drew a bright
   line along the horizon on WARP; depth is well conditioned in half floats, interpolates in
   log space, and a segment's transmittance becomes `exp` of a difference (no division);
   (c) *ray integrals crowd samples toward dense air* (`t = d s^2`, or its mirror toward the
   ground): uniform steps of a 600 km horizon ray are ten times the haze's scale height (16 %
   error).

<!-- -->

6. **Receiver-plane depth bias for PCF** (amends ADR 0023's normal-offset-only bias): each 3x3
   tap compares against the receiver's own depth at that tap, from the screen derivatives of
   the view position. A single depth across the kernel drew rings of acne on open ground under
   a 40-degree sun (found by the atmosphere goldens). Every shadow guard is unchanged and
   green.

7. **The perf gate budgets GPU time per device class and CPU time as a calibrated ratio.**
   `tests/perf/budgets.ron` names every budget; GPU rows carry baselines for the RTX 3080 and
   for WARP (the GPU-less Windows CI leg), allowed `max(base x 1.5, base + slack[class])`, the
   slack being that class's timestamp quantisation and run-to-run spread (RTX 3080: 0.003 ms,
   measured over 14 runs; WARP: 0.05 ms). The first cut used one 0.05 ms slack for every class,
   which on the RTX 3080 (passes of 0.01-0.034 ms) let a pass grow 2.5-6x before its own row
   failed (independent verifier); with the per-class slack every RTX 3080 row is held to the
   1.5x band. CPU rows
   are measured / (an in-process lattice-fBm calibration workload), allowed `ratio x 1.5 +
   0.002`, so one number holds on any machine; counters are exact maxima; coverage is checked
   both ways (an unmeasured row and an unbudgeted measurement both fail). A device of another
   class (lavapipe on the ubuntu leg, another GPU) has no GPU baseline: its GPU rows print NOT
   GATED (gate row `C-perf-gate-linux-leg` AWAITING) while its CPU and counter rows gate. The
   gate runs alone under nextest (`threads-required = "num-cpus"`). Its control injects a real
   regression (a hidden knob evaluates the sky 48 more times per pixel) and checks it against
   the clean run's own numbers, so it bites on every device; a second control adds sky evaluations sized
   per class to 1.75x (RTX 3080, +8 % on the frame) and 1.85x (WARP) of the pass and must fail
   the sky pass's row against the committed baseline; and the gate itself refuses any slack wider than
   `band x baseline` for any row of any class (control: the first cut's blanket 0.05 ms is
   refused). The knob's first repeat now offsets the view direction (a zero offset let WARP's
   compiler merge it with the real evaluation).
9. **The default haze is continental** (optical depth 0.08, Angstrom 1.3), not Bruneton's
   very clean reference (0.005): a whitened horizon and a realistic zenith (rule 1).

   The default is Earth's air.

## Consequences

- The atmosphere adds ~0.02 ms (sky) and a few lookups per shaded fragment at 1280x720 on the
  RTX 3080.
