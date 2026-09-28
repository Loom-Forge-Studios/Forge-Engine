//! Sun shadows: **cascaded shadow maps**, the documented fallback for virtual shadow maps
//! (ADR 0023, Ch.10 §10.5).
//!
//! Why not VSM at M1: a virtual shadow map is a sparse 16k² depth texture whose pages are
//! resident only where a visible receiver needs them. wgpu exposes no sparse (tiled)
//! resources, so it would be emulated — a page table, a GPU feedback pass marking needed
//! pages, a readback or GPU-driven allocator into a physical page atlas, per-page culling
//! and caching. That is a milestone of its own; the M1 exit ("a correctly placed sun")
//! needs stable, correct sun shadows now. The cascades below are the standard, well
//! understood alternative; VSM stays owed (DoD M1-7).
//!
//! Everything is computed in `f64`, camera-relative:
//!
//! * **Splits** — the practical scheme (`lambda` between logarithmic and uniform) from
//!   [`CascadeSettings::near`] to [`CascadeSettings::max_distance`].
//! * **Fit** — each cascade is the bounding sphere of its slice of the view frustum. Its
//!   radius depends only on the field of view, aspect and split depths, so it does not
//!   change as the camera turns or moves: the texel size is constant.
//! * **Snapping** — the sphere's centre is snapped to the shadow-texel grid **in the camera
//!   frame's own coordinates** (the camera's `FramePos::local` plus the offset, `f64`,
//!   read inside [`fit`] only), then made camera-relative
//!   again. The grid is fixed to the ground, not to the camera, so a moving camera does not
//!   make shadow edges crawl (`test_shadows::cascade_texels_are_world_anchored`).
//! * **Depth** — orthographic along the light, reversed (1 = nearest the light), extended
//!   toward the light by `caster_reach` so casters outside the sphere still shadow into it.

use forge_num::{DQuat, DVec3, det};

use crate::camera::Camera;
use crate::last_mile::FrameOffset;

/// How the sun's shadow cascades are laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeSettings {
    /// Number of cascades, 1..=4.
    pub count: u32,
    /// Resolution of each cascade (square), texels.
    pub resolution: u32,
    /// Depth where the first cascade starts, metres.
    pub near: f64,
    /// Depth beyond which nothing is shadowed, metres.
    pub max_distance: f64,
    /// 0 = uniform splits, 1 = logarithmic.
    pub split_lambda: f64,
    /// How far toward the light, beyond a cascade's sphere, casters are still captured, m.
    pub caster_reach: f64,
}

impl Default for CascadeSettings {
    fn default() -> Self {
        Self {
            count: 4,
            resolution: 2048,
            near: 0.1,
            max_distance: 500.0,
            split_lambda: 0.8,
            caster_reach: 1000.0,
        }
    }
}

/// One cascade, ready to upload.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cascade {
    /// View depth where this cascade ends.
    pub split: f64,
    /// Centre of the (snapped) sphere, camera-relative, **camera-frame axes**.
    pub offset: FrameOffset,
    /// Radius, metres.
    pub radius: f64,
    /// World size of one shadow texel, metres.
    pub texel: f64,
    /// Light-space basis in camera-frame axes: `x`, `y`, and `toward_light`.
    pub basis: [DVec3; 3],
    /// Light-space depth range `(min, max)` along `toward_light` relative to `offset`.
    pub depth_range: (f64, f64),
}

impl Cascade {
    /// Light-space coordinates of a camera-relative point given in camera-frame axes:
    /// `(x, y)` in `-1..1` across the map and depth in `0..1` (1 nearest the light).
    #[must_use]
    pub fn project(&self, p: FrameOffset) -> DVec3 {
        let q = p.0 - self.offset.0;
        let (lo, hi) = self.depth_range;
        DVec3::new(
            q.dot(self.basis[0]) / self.radius,
            q.dot(self.basis[1]) / self.radius,
            (q.dot(self.basis[2]) - lo) / (hi - lo),
        )
    }

    /// The matrix taking **view-space** points to the cascade's clip space (column-major
    /// `f64`; `w` stays 1). `view_to_frame` is the camera orientation.
    #[must_use]
    pub fn matrix_from_view(&self, view_to_frame: DQuat) -> [[f64; 4]; 4] {
        let (lo, hi) = self.depth_range;
        let span = hi - lo;
        // Row i of the linear part is basis_i (in view axes) / scale_i.
        let rows = [
            view_to_frame.inverse_rotate(self.basis[0]) / self.radius,
            view_to_frame.inverse_rotate(self.basis[1]) / self.radius,
            view_to_frame.inverse_rotate(self.basis[2]) / span,
        ];
        let t = [
            -self.offset.0.dot(self.basis[0]) / self.radius,
            -self.offset.0.dot(self.basis[1]) / self.radius,
            (-self.offset.0.dot(self.basis[2]) - lo) / span,
        ];
        [
            [rows[0].x, rows[1].x, rows[2].x, 0.0],
            [rows[0].y, rows[1].y, rows[2].y, 0.0],
            [rows[0].z, rows[1].z, rows[2].z, 0.0],
            [t[0], t[1], t[2], 1.0],
        ]
    }

    /// Could a caster sphere (camera-relative, camera-frame axes) shadow into this cascade?
    #[must_use]
    pub fn captures(&self, c: FrameOffset, r: f64) -> bool {
        let q = c.0 - self.offset.0;
        let (lo, hi) = self.depth_range;
        let (x, y, z) = (
            q.dot(self.basis[0]),
            q.dot(self.basis[1]),
            q.dot(self.basis[2]),
        );
        x.abs() <= self.radius + r && y.abs() <= self.radius + r && z >= lo - r && z <= hi + r
    }
}

/// Split depths (the far end of each cascade).
#[must_use]
pub fn splits(s: &CascadeSettings) -> Vec<f64> {
    (1..=s.count.clamp(1, 4)).map(|i| split(s, i)).collect()
}

/// The far end of cascade `i` (1-based) — [`splits`] without the `Vec`.
fn split(s: &CascadeSettings, i: u32) -> f64 {
    let n = s.count.clamp(1, 4);
    let (zn, zf) = (s.near, s.max_distance);
    let f = f64::from(i) / f64::from(n);
    let log = zn * det::pow(zf / zn, f);
    let uni = zn + (zf - zn) * f;
    s.split_lambda * log + (1.0 - s.split_lambda) * uni
}

/// An orthonormal basis with `z = toward_light`.
fn light_basis(toward_light: DVec3) -> [DVec3; 3] {
    let l = toward_light.try_normalize().unwrap_or(DVec3::Z);
    let reference = if l.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    let x = l.cross(reference).try_normalize().unwrap_or(DVec3::X);
    let y = l.cross(x);
    [x, y, l]
}

/// Fit the cascades for `camera` (field of view, orientation, and — as the anchor for texel
/// snapping — its position in its own frame) on a viewport of `aspect` width/height.
/// `toward_light` is in the camera frame's axes.
#[must_use]
pub fn fit(
    s: &CascadeSettings,
    camera: &Camera,
    aspect: f64,
    toward_light: DVec3,
    snap: bool,
) -> Vec<Cascade> {
    let mut out = Vec::new();
    fit_into(s, camera, aspect, toward_light, snap, &mut out);
    out
}

/// [`fit`] into a caller-kept `Vec` (cleared first): frame preparation reuses one across
/// frames, so fitting allocates nothing once warm.
pub fn fit_into(
    s: &CascadeSettings,
    camera: &Camera,
    aspect: f64,
    toward_light: DVec3,
    snap: bool,
    out: &mut Vec<Cascade>,
) {
    out.clear();
    let basis = light_basis(toward_light);
    let tan_half_fov = camera.tan_half_fov();
    let view_to_frame = camera.orientation;
    // The snapping anchor: the camera's own position in its frame. It never leaves this
    // function; what comes out is camera-relative.
    let anchor = camera.pos.local;
    // Squared lateral extent per unit depth at the frustum's corners.
    let k2 = tan_half_fov * tan_half_fov * (1.0 + aspect * aspect);
    let mut d0 = s.near;
    for i in 1..=s.count.clamp(1, 4) {
        let d1 = split(s, i);
        let mut dc = 0.5 * (1.0 + k2) * (d0 + d1);
        if dc > d1 {
            dc = d1;
        }
        let radius = det::sqrt((d1 - dc) * (d1 - dc) + k2 * d1 * d1);
        let texel = 2.0 * radius / f64::from(s.resolution.max(1));
        let mut center = view_to_frame.rotate(DVec3::new(0.0, 0.0, -dc));
        if snap {
            for axis in [basis[0], basis[1]] {
                let a = (anchor + center).dot(axis);
                let snapped = (a / texel).floor() * texel;
                center += axis * (snapped - a);
            }
        }
        out.push(Cascade {
            split: d1,
            offset: FrameOffset(center),
            radius,
            texel,
            basis,
            depth_range: (-radius, radius + s.caster_reach),
        });
        d0 = d1;
    }
}
