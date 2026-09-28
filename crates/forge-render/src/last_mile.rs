//! **The last mile** (Ch.2.4, M1-3): everything is resolved into the camera frame in `f64`,
//! made camera-relative in `f64`, and only then narrowed to `f32`.
//!
//! This module is the **only** place in the engine where a position becomes `f32`:
//! `tests/liveness/test_no_f32_below_render.rs` asserts that no crate below `forge-render`
//! holds an `f32`, and that inside `forge-render` every narrowing (`as f32`, `f32::from`) is
//! in this file. There is no other path to the GPU.
//!
//! Why it matters: an `f32` holds 24 bits of mantissa, so a coordinate 6,371 km from the
//! frame's origin is quantised to 0.5 m. Narrow the camera and
//! the object separately and subtract, and a character 3 m away jitters by half a metre as
//! the camera moves ("precision shimmer"). Subtract in `f64` first and the difference — a few
//! metres — narrows to within ~0.2 µm. [`mutants::naive_model_view`] is the wrong way, kept
//! only so `test_last_mile` can prove its guard fails on it (W2).

use forge_frames::{DQuat, DVec3, FrameError, FrameId, FramePos, FrameResolver, Tick};

use crate::camera::Camera;
use crate::error::RenderError;

/// A displacement from the camera, in the camera's **view axes** (`+x` right, `+y` up,
/// `-z` forward), in `f64`. What every position becomes before it is narrowed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewOffset(pub DVec3);

impl ViewOffset {
    /// Distance along the view direction (positive in front of the camera).
    #[must_use]
    pub fn depth(self) -> f64 {
        -self.0.z
    }

    /// The same displacement in the camera frame's axes (`view_to_frame` is
    /// [`Camera::orientation`]).
    #[must_use]
    pub fn to_frame(self, view_to_frame: DQuat) -> FrameOffset {
        FrameOffset(view_to_frame.rotate(self.0))
    }
}

/// A displacement from the camera in the axes of the **camera's frame** (not the view axes),
/// in `f64`. Shadow cascades live here: their light-space basis is fixed to the frame, so it
/// does not turn with the camera. [`ViewOffset`] and `FrameOffset` differ only by the camera
/// orientation; two types keep a point from being read in the wrong axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameOffset(pub DVec3);

impl FrameOffset {
    /// The same displacement in view axes (`view_to_frame` is [`Camera::orientation`]).
    #[must_use]
    pub fn to_view(self, view_to_frame: DQuat) -> ViewOffset {
        ViewOffset(view_to_frame.inverse_rotate(self.0))
    }
}

/// The camera resolved at one tick: turns [`FramePos`]es and frame-relative directions into
/// [`ViewOffset`]s and view-axis directions, all in `f64`.
pub struct CameraView<'t> {
    tree: &'t dyn FrameResolver,
    t: Tick,
    camera: Camera,
    /// Camera frame axes -> root axes.
    cam_to_root: DQuat,
    root: FrameId,
    /// Frame -> view rotations already resolved this frame. Inline (a scene touches a handful
    /// of frames); beyond `ROT_CACHE` entries a rotation is recomputed, never allocated.
    rot_cache: [(FrameId, DQuat); ROT_CACHE],
    rot_cached: usize,
}

const ROT_CACHE: usize = 16;

impl<'t> CameraView<'t> {
    /// Resolve `camera` in `tree` at `t`.
    pub fn new(tree: &'t dyn FrameResolver, camera: &Camera, t: Tick) -> Result<Self, RenderError> {
        camera.validate()?;
        let (cam_to_root, root) = tree.rotation_to_root(camera.pos.frame, t)?;
        Ok(Self {
            tree,
            t,
            camera: *camera,
            cam_to_root,
            root,
            rot_cache: [(FrameId(0), DQuat::IDENTITY); ROT_CACHE],
            rot_cached: 0,
        })
    }

    /// The camera.
    #[must_use]
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// Where `p` is relative to the camera, in view axes: resolve into the camera's frame,
    /// subtract the camera's position (both `f64`), rotate into view axes.
    pub fn offset(&self, p: FramePos) -> Result<ViewOffset, RenderError> {
        let in_cam = self.tree.resolve(p, self.camera.pos.frame, self.t)?;
        let d = in_cam.local - self.camera.pos.local;
        Ok(ViewOffset(self.camera.orientation.inverse_rotate(d)))
    }

    /// The rotation taking `frame`'s axes to the view axes.
    pub fn rotation_from(&mut self, frame: FrameId) -> Result<DQuat, RenderError> {
        if let Some((_, q)) = self.rot_cache[..self.rot_cached]
            .iter()
            .find(|(f, _)| *f == frame)
        {
            return Ok(*q);
        }
        let (f_to_root, root) = self.tree.rotation_to_root(frame, self.t)?;
        if root != self.root {
            return Err(FrameError::Disconnected {
                from: frame,
                to: self.camera.pos.frame,
            }
            .into());
        }
        // view <- camera frame <- root <- frame
        let q = self.camera.orientation.conjugate() * self.cam_to_root.conjugate() * f_to_root;
        if self.rot_cached < ROT_CACHE {
            self.rot_cache[self.rot_cached] = (frame, q);
            self.rot_cached += 1;
        }
        Ok(q)
    }

    /// A direction given in `frame`'s axes, in view axes.
    pub fn direction(&mut self, frame: FrameId, v: DVec3) -> Result<DVec3, RenderError> {
        Ok(self.rotation_from(frame)?.rotate(v))
    }
}

/// Narrow one `f64` to `f32`. Callers hand in camera-relative or non-positional values only.
#[inline]
#[must_use]
pub(crate) fn narrow(x: f64) -> f32 {
    x as f32
}

/// Narrow a vector.
#[inline]
#[must_use]
pub(crate) fn narrow3(v: DVec3) -> [f32; 3] {
    [narrow(v.x), narrow(v.y), narrow(v.z)]
}

/// Widen back (for the precision tests and the naive mutant).
#[inline]
#[must_use]
pub(crate) fn widen3(v: [f32; 3]) -> DVec3 {
    DVec3::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2]))
}

/// A column-major 4x4 `f64` matrix narrowed to `f32`.
#[must_use]
pub(crate) fn mat4(m: &[[f64; 4]; 4]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for (c, col) in m.iter().enumerate() {
        for (r, v) in col.iter().enumerate() {
            out[c * 4 + r] = narrow(*v);
        }
    }
    out
}

/// The model-view matrix of an instance (column-major, `f32`): object axes scaled by `scale`
/// and rotated into view axes by `rot`, then placed at the camera-relative `offset`. The
/// translation column is a camera-relative distance — metres, not megametres — so it narrows
/// exactly enough for any object that is on screen.
#[must_use]
pub fn model_view(rot: DQuat, scale: DVec3, offset: ViewOffset) -> [f32; 16] {
    let cx = rot.rotate(DVec3::X) * scale.x;
    let cy = rot.rotate(DVec3::Y) * scale.y;
    let cz = rot.rotate(DVec3::Z) * scale.z;
    mat4(&[
        [cx.x, cx.y, cx.z, 0.0],
        [cy.x, cy.y, cy.z, 0.0],
        [cz.x, cz.y, cz.z, 0.0],
        [offset.0.x, offset.0.y, offset.0.z, 1.0],
    ])
}

/// The normal matrix (`R S^-1`, the inverse transpose of `R S`) as three `vec4` columns.
#[must_use]
pub fn normal_matrix(rot: DQuat, scale: DVec3) -> [f32; 12] {
    let cx = rot.rotate(DVec3::X) / scale.x;
    let cy = rot.rotate(DVec3::Y) / scale.y;
    let cz = rot.rotate(DVec3::Z) / scale.z;
    let [a, b, c] = narrow3(cx);
    let [d, e, f] = narrow3(cy);
    let [g, h, i] = narrow3(cz);
    [a, b, c, 0.0, d, e, f, 0.0, g, h, i, 0.0]
}

/// Apply a narrowed model-view matrix to a narrowed object-space vertex in `f32`, the way
/// the vertex shader does (for the precision tests).
#[must_use]
pub fn apply_f32(m: &[f32; 16], v: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r];
    }
    out
}

/// Narrow an object-space vertex coordinate for upload.
#[must_use]
pub fn vertex(v: DVec3) -> [f32; 3] {
    narrow3(v)
}

/// W2 positive controls for `test_last_mile`. **Never call from engine code.**
#[doc(hidden)]
pub mod mutants {
    use super::*;

    /// The model-view matrix the way an engine with `f32` world positions builds it: the
    /// object's and the camera's frame-local positions are each narrowed to `f32` and then
    /// subtracted. At 6.371e6 m from the frame origin each is quantised to 0.5 m.
    pub fn naive_model_view(
        view: &CameraView<'_>,
        obj: FramePos,
        rot: DQuat,
        scale: DVec3,
    ) -> Result<[f32; 16], RenderError> {
        let cam = view.camera();
        let in_cam = view.tree.resolve(obj, cam.pos.frame, view.t)?;
        let o = narrow3(in_cam.local);
        let c = narrow3(cam.pos.local);
        let diff = [o[0] - c[0], o[1] - c[1], o[2] - c[2]];
        let offset = ViewOffset(cam.orientation.inverse_rotate(widen3(diff)));
        Ok(model_view(rot, scale, offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::WorldFrame;

    #[test]
    fn offset_subtracts_in_f64_before_narrowing() {
        let tree = WorldFrame;
        let root = FrameId::WORLD;
        let r = 6.371e6;
        let cam = Camera::looking(
            FramePos::new(root, DVec3::new(r, 0.0, 0.0)),
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::Y,
            1.0,
        )
        .expect("cam");
        let view = CameraView::new(&tree, &cam, Tick(0)).expect("view");
        let o = view
            .offset(FramePos::new(root, DVec3::new(r + 0.123_456_7, 0.0, -3.0)))
            .expect("offset");
        let n = narrow3(o.0);
        assert!((f64::from(n[0]) - 0.123_456_7).abs() < 1e-7, "{n:?}");
        assert!((o.depth() - 3.0).abs() < 1e-9);
    }
}
