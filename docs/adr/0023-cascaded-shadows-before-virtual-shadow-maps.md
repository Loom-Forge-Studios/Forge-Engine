# ADR 0023 — Ship cascaded sun shadows now; virtual shadow maps stay owed

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.10 contract ("virtual shadow maps") and §10.5, M1-7, M1 exit criterion

## Context

Ch.10 and DoD M1-7 name virtual shadow maps (VSM). The work package allows "a documented
cascaded fallback with the reason". A VSM is a very large (typically 16k²) depth texture
of which only the pages a visible receiver samples are resident and rendered.

wgpu (v30) exposes **no sparse / tiled resources** on any backend. A VSM therefore has to be
emulated: a page table texture, a GPU pass that marks the pages visible receivers need, an
allocator mapping them into a physical page atlas (GPU-driven, or through a readback with a
frame of latency), per-page caster culling and invalidation, and caching of static pages
across frames. Each is a subsystem with its own failure modes; together they are larger
than the rest of WP-09.

## Decision

Sun shadows are **cascaded shadow maps** (`forge_render::shadow`): up to four cascades in
one `Depth32Float` array texture (2048² by default), practical split scheme to 500 m,
bounding-sphere fit (constant texel size under rotation), centre snapped to the texel grid
in the camera frame's own `f64` coordinates (no crawl when the camera moves), reversed
orthographic depth extended 1 km toward the light for off-screen casters, normal-offset
bias, 3x3 hardware PCF. Punctual lights do not cast shadows yet.

M1-7 is recorded **Blocked** in `docs/plan/dod-status.ron` for the VSM half, naming the
missing capability; everything else in M1-7 is built and guarded. Gate row `C-vsm` is
`Unbuilt` with the same reason.

## Why — the owner's two rules

1. **Better for the user:** correct, stable sun shadows at the M1 exit ("a correctly
   placed sun") instead of none while an emulated VSM is built.
2. **Faster engine:** four 2048² cascades cost ~0.5 ms GPU for 2,500 casters at 1080p
   (measured, Ch.10 §10.9), with one draw per mesh per cascade.

## Alternatives rejected

- **Build the emulated VSM now:** the page-feedback loop needs either GPU-driven allocation
  (atomics, indirect draws per page) or a readback with a frame of latency and visible
  popping; either is a milestone of its own and blocks every other M1-7 item.
- **Wait for sparse resources in wgpu:** no delivery date; would leave M1 without shadows.

## Consequences

- When the VSM is built it replaces `shadow.cascade*` passes; the fragment shader's
  `sun_shadow` is the only consumer, and `test_shadows`' floor-shadow check is its
  acceptance test to keep.

## Restated (WP-18, ADR 0028)

The reason has moved: a VSM does not need sparse resources to be built on the RTX 3080 (GPU page
marking with storage atomics, a GPU allocator, per-page caster routing with indirect draws and
clip distances). It stays owed because on M1's content it buys neither owner rule — the four
cascades cost 0.116 ms of the 0.18 ms M1 frame (perf gate, 1280x720), which a VSM's fixed passes
alone would match, and its gains need dense distant casters M1 does not have. See ADR 0028,
decision 6.
