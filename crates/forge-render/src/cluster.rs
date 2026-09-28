//! Clustered light culling (Ch.10 §10.3): the view frustum is cut into `x * y` screen tiles
//! and `z` logarithmic depth slices; every punctual light is binned, in `f64` view space,
//! into exactly the clusters its sphere of influence touches. The fragment shader finds its
//! cluster from its pixel and view depth and loops over that cluster's lights only.
//!
//! Binning is exact sphere-vs-cluster-box, **conservative** (each cluster box is padded by
//! a pixel and a sliver of depth so a fragment the GPU puts in a neighbouring cluster through
//! rounding still finds the light), and checked against brute force by `test_clustered_lighting`.
//! It runs on the CPU (ADR 0023): at M1 light counts (hundreds to a few thousand) the bin is
//! well under a millisecond, deterministic, and needs no compute pass or GPU-side list
//! allocation; the compute-shader binner is the follow-up when counts outgrow it.

use forge_num::{DVec3, det};

use crate::last_mile::ViewOffset;

/// The cluster grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterGrid {
    /// Screen tiles across.
    pub x: u32,
    /// Screen tiles down.
    pub y: u32,
    /// Depth slices.
    pub z: u32,
    /// Nearest clustered depth, metres (the nearest depth pass's near plane).
    pub near: f64,
    /// Farthest clustered depth, metres: punctual lights reach fragments up to here (1e4
    /// km — beyond it a punctual light is sub-pixel).
    pub far: f64,
}

impl Default for ClusterGrid {
    fn default() -> Self {
        Self {
            x: 16,
            y: 9,
            z: 48,
            near: crate::depth::ViewDepth::NEAREST,
            far: 1e7,
        }
    }
}

/// What binning needs to know about the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterView {
    /// Target width, pixels.
    pub width: u32,
    /// Target height, pixels.
    pub height: u32,
    /// `tan(fov_y / 2)`.
    pub tan_half_fov: f64,
}

/// A light's sphere of influence in view space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightSphere {
    /// Centre, view axes (camera-relative), metres.
    pub offset: ViewOffset,
    /// Radius, metres.
    pub radius: f64,
}

/// The binned lists: per cluster `(offset, count)` into `indices`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clusters {
    /// `(offset, count)` per cluster, index `(slice * y + tile_y) * x + tile_x`.
    pub ranges: Vec<[u32; 2]>,
    /// Light indices.
    pub indices: Vec<u32>,
}

impl ClusterGrid {
    /// Tile size in pixels `(width, height)`.
    #[must_use]
    pub fn tile_px(&self, v: &ClusterView) -> (u32, u32) {
        (
            v.width.div_ceil(self.x.max(1)),
            v.height.div_ceil(self.y.max(1)),
        )
    }

    /// `slices / log2(far / near)`: `slice = floor(log2(depth / near) * scale)`.
    #[must_use]
    pub fn slice_scale(&self) -> f64 {
        f64::from(self.z) / log2(self.far / self.near)
    }

    /// The depth where slice `k` begins.
    #[must_use]
    pub fn slice_depth(&self, k: u32) -> f64 {
        self.near * det::pow(2.0, f64::from(k) / self.slice_scale())
    }

    /// The slice of view depth `d` (clamped into the grid).
    #[must_use]
    pub fn slice_of(&self, d: f64) -> u32 {
        if d <= self.near {
            return 0;
        }
        let s = (log2(d / self.near) * self.slice_scale()).floor();
        (s.max(0.0) as u32).min(self.z - 1)
    }

    /// The cluster index of a fragment at pixel `(px, py)` and view depth `d`, the way the
    /// shader computes it. `None` outside the clustered depth range.
    #[must_use]
    pub fn cluster_of(&self, v: &ClusterView, px: f64, py: f64, d: f64) -> Option<usize> {
        if !(d >= self.near && d < self.far) {
            return None;
        }
        let (tw, th) = self.tile_px(v);
        let tx = ((px / f64::from(tw)) as u32).min(self.x - 1);
        let ty = ((py / f64::from(th)) as u32).min(self.y - 1);
        let s = self.slice_of(d);
        Some(((s * self.y + ty) * self.x + tx) as usize)
    }

    /// Refresh the padded per-tile lateral slopes (x and y extent per metre of depth) and
    /// per-slice depth ranges in `scratch`, if the grid or view changed since they were
    /// built: a cluster's box is then a handful of multiplies. The tables are refilled in
    /// place, so a refresh reuses their storage.
    fn refresh_tables(&self, v: &ClusterView, scratch: &mut ClusterScratch) {
        if scratch.tables_for == Some((*self, *v)) {
            return;
        }
        let (tw, th) = self.tile_px(v);
        let (w, h) = (f64::from(v.width), f64::from(v.height));
        let aspect = w / h;
        let t = v.tan_half_fov;
        // One pixel of padding on each side, and 0.1 % of depth.
        scratch.xs.clear();
        scratch.xs.extend((0..self.x).map(|tx| {
            let x0 = f64::from(tx * tw) - 1.0;
            let x1 = f64::from((tx + 1) * tw) + 1.0;
            (
                (2.0 * x0 / w - 1.0) * aspect * t,
                (2.0 * x1 / w - 1.0) * aspect * t,
            )
        }));
        scratch.ys.clear();
        scratch.ys.extend((0..self.y).map(|ty| {
            let y0 = f64::from(ty * th) - 1.0;
            let y1 = f64::from((ty + 1) * th) + 1.0;
            ((1.0 - 2.0 * y1 / h) * t, (1.0 - 2.0 * y0 / h) * t)
        }));
        scratch.ds.clear();
        scratch.ds.extend(
            (0..self.z).map(|s| (self.slice_depth(s) * 0.999, self.slice_depth(s + 1) * 1.001)),
        );
        scratch.tables_for = Some((*self, *v));
    }

    /// Bin `lights` (view space) into clusters, allocating fresh storage. Convenient for
    /// tools and tests; the renderer uses [`ClusterGrid::bin_into`] with buffers it keeps.
    #[must_use]
    pub fn bin(&self, v: &ClusterView, lights: &[LightSphere]) -> Clusters {
        let mut out = Clusters::default();
        self.bin_into(v, lights, &mut ClusterScratch::default(), &mut out);
        out
    }

    /// Bin `lights` (view space) into `out`, reusing `scratch` and `out`'s storage: once
    /// their capacity has grown to the scene's size, a frame allocates nothing.
    ///
    /// One geometric pass records every `(cluster, light)` hit; a counting pass sizes each
    /// cluster; a prefix sum places the clusters in `out.indices`; a scatter pass fills them.
    /// Hits are recorded in light order and the scatter is stable, so each cluster lists its
    /// lights in ascending index order.
    pub fn bin_into(
        &self,
        v: &ClusterView,
        lights: &[LightSphere],
        scratch: &mut ClusterScratch,
        out: &mut Clusters,
    ) {
        let n = (self.x * self.y * self.z) as usize;
        let (w, h) = (f64::from(v.width), f64::from(v.height));
        let aspect = w / h;
        let t = v.tan_half_fov;
        let (tw, th) = self.tile_px(v);
        self.refresh_tables(v, scratch);
        let ClusterScratch {
            xs, ys, ds, hits, ..
        } = scratch;
        hits.clear();
        for (li, l) in lights.iter().enumerate() {
            let d_lo = -l.offset.0.z - l.radius;
            let d_hi = -l.offset.0.z + l.radius;
            if d_hi < self.near || d_lo >= self.far || l.radius.is_nan() || l.radius <= 0.0 {
                continue;
            }
            let s0 = self.slice_of(d_lo.max(self.near));
            let s1 = self.slice_of(d_hi.min(self.far));
            // Candidate tiles: the projection of the sphere's bounding box, when all of it is
            // in front of the eye; otherwise every tile.
            let (mut tx0, mut tx1, mut ty0, mut ty1) = (0, self.x - 1, 0, self.y - 1);
            if d_lo > 1e-6 {
                let (mut nx0, mut nx1, mut ny0, mut ny1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
                for sx in [-1.0, 1.0] {
                    for sy in [-1.0, 1.0] {
                        for sz in [-1.0, 1.0] {
                            let p = l.offset.0 + DVec3::new(sx, sy, sz) * l.radius;
                            let d = -p.z;
                            let nx = p.x / (aspect * t * d);
                            let ny = p.y / (t * d);
                            nx0 = nx0.min(nx);
                            nx1 = nx1.max(nx);
                            ny0 = ny0.min(ny);
                            ny1 = ny1.max(ny);
                        }
                    }
                }
                let px0 = (nx0 + 1.0) * 0.5 * w;
                let px1 = (nx1 + 1.0) * 0.5 * w;
                let py0 = (1.0 - ny1) * 0.5 * h;
                let py1 = (1.0 - ny0) * 0.5 * h;
                if px1 < -1.0 || px0 > w + 1.0 || py1 < -1.0 || py0 > h + 1.0 {
                    continue;
                }
                let tile = |p: f64, size: u32, count: u32| {
                    ((p / f64::from(size)).floor().max(0.0) as u32).min(count - 1)
                };
                tx0 = tile(px0 - 1.0, tw, self.x);
                tx1 = tile(px1 + 1.0, tw, self.x);
                ty0 = tile(py0 - 1.0, th, self.y);
                ty1 = tile(py1 + 1.0, th, self.y);
            }
            for s in s0..=s1 {
                let (d0, d1) = ds[s as usize];
                for ty in ty0..=ty1 {
                    let (ky0, ky1) = ys[ty as usize];
                    let (y_lo, y_hi) = ((ky0 * d0).min(ky0 * d1), (ky1 * d0).max(ky1 * d1));
                    for tx in tx0..=tx1 {
                        let (kx0, kx1) = xs[tx as usize];
                        let lo = DVec3::new((kx0 * d0).min(kx0 * d1), y_lo, -d1);
                        let hi = DVec3::new((kx1 * d0).max(kx1 * d1), y_hi, -d0);
                        if sphere_box(l.offset.0, l.radius, lo, hi) {
                            hits.push([(s * self.y + ty) * self.x + tx, li as u32]);
                        }
                    }
                }
            }
        }
        // Count per cluster (`ranges[c][1]`).
        out.ranges.clear();
        out.ranges.resize(n, [0, 0]);
        for &[c, _] in hits.iter() {
            out.ranges[c as usize][1] += 1;
        }
        // Prefix sum: each cluster's offset; then reset the counts to act as write cursors.
        let mut offset = 0u32;
        for r in &mut out.ranges {
            r[0] = offset;
            offset += r[1];
            r[1] = 0;
        }
        // Scatter (stable: hits are in light order); the cursors end at the counts.
        out.indices.clear();
        out.indices.resize(hits.len(), 0);
        for &[c, li] in hits.iter() {
            let r = &mut out.ranges[c as usize];
            out.indices[(r[0] + r[1]) as usize] = li;
            r[1] += 1;
        }
    }
}

/// Reusable working storage for [`ClusterGrid::bin_into`]: the per-tile and per-slice
/// tables (rebuilt only when the grid or view changes) and the hit list. Keep one alive
/// across frames — the renderer does — so binning allocates nothing in steady state.
#[derive(Clone, Debug, Default)]
pub struct ClusterScratch {
    tables_for: Option<(ClusterGrid, ClusterView)>,
    xs: Vec<(f64, f64)>,
    ys: Vec<(f64, f64)>,
    ds: Vec<(f64, f64)>,
    /// `[cluster, light]` per sphere-box hit.
    hits: Vec<[u32; 2]>,
}

fn log2(x: f64) -> f64 {
    det::ln(x) / core::f64::consts::LN_2
}

/// Does the sphere touch the box?
fn sphere_box(c: DVec3, r: f64, lo: DVec3, hi: DVec3) -> bool {
    let q = DVec3::new(
        c.x.clamp(lo.x, hi.x),
        c.y.clamp(lo.y, hi.y),
        c.z.clamp(lo.z, hi.z),
    );
    (q - c).length_squared() <= r * r
}

/// W2 positive control for `test_clustered_lighting`. **Never call from engine code.**
#[doc(hidden)]
pub mod mutants {
    use super::*;

    /// Binning with every light's radius halved: lights reach fragments their clusters do
    /// not list.
    #[must_use]
    pub fn bin_shrunk(grid: &ClusterGrid, v: &ClusterView, lights: &[LightSphere]) -> Clusters {
        let shrunk: Vec<LightSphere> = lights
            .iter()
            .map(|l| LightSphere {
                offset: l.offset,
                radius: l.radius * 0.5,
            })
            .collect();
        grid.bin(v, &shrunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lights(n: u32, shift: f64) -> Vec<LightSphere> {
        (0..n)
            .map(|i| {
                let f = f64::from(i);
                LightSphere {
                    offset: ViewOffset(DVec3::new(
                        det::sin(f * 0.37) * 20.0 + shift,
                        det::cos(f * 0.21) * 8.0,
                        -3.0 - f * 0.9,
                    )),
                    radius: 2.0 + det::sin(f * 0.13).abs() * 6.0,
                }
            })
            .collect()
    }

    fn view(width: u32) -> ClusterView {
        ClusterView {
            width,
            height: 1080,
            tan_half_fov: 0.5,
        }
    }

    /// Binning into kept buffers matches a fresh bin, and once warm allocates nothing: the
    /// storage of the output and the scratch is the same allocation, with the same capacity,
    /// frame after frame, as long as the scene does not grow.
    #[test]
    fn warm_bin_into_reuses_its_storage_and_matches_a_fresh_bin() {
        let grid = ClusterGrid::default();
        let mut scratch = ClusterScratch::default();
        let mut out = Clusters::default();
        grid.bin_into(&view(1920), &lights(256, 0.0), &mut scratch, &mut out);
        assert!(!out.indices.is_empty());
        let fingerprint = |s: &ClusterScratch, o: &Clusters| {
            (
                o.ranges.as_ptr(),
                o.ranges.capacity(),
                o.indices.as_ptr(),
                o.indices.capacity(),
                s.hits.as_ptr(),
                s.hits.capacity(),
                s.xs.as_ptr(),
                s.ys.as_ptr(),
                s.ds.as_ptr(),
            )
        };
        let warm = fingerprint(&scratch, &out);
        // Same light count, moved lights (fewer or equal hits): no reallocation.
        for k in 0..4 {
            let ls = lights(256, 0.25 * f64::from(k));
            grid.bin_into(&view(1920), &ls, &mut scratch, &mut out);
            assert_eq!(out, grid.bin(&view(1920), &ls), "frame {k}");
            if out.indices.len() <= warm.3 {
                assert_eq!(fingerprint(&scratch, &out), warm, "frame {k} reallocated");
            }
        }
        // A view change rebuilds the tables (in place) and still matches a fresh bin.
        let ls = lights(256, 0.0);
        grid.bin_into(&view(1280), &ls, &mut scratch, &mut out);
        assert_eq!(out, grid.bin(&view(1280), &ls));
    }

    /// Each cluster lists its lights in ascending order, and ranges tile `indices` exactly.
    #[test]
    fn ranges_are_contiguous_and_lists_sorted() {
        let c = ClusterGrid::default().bin(&view(1920), &lights(300, 0.0));
        let mut next = 0u32;
        for r in &c.ranges {
            assert_eq!(r[0], next);
            let list = &c.indices[r[0] as usize..(r[0] + r[1]) as usize];
            assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
            next += r[1];
        }
        assert_eq!(next as usize, c.indices.len());
    }
}
