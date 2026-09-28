//! The camera: a [`FramePos`], an orientation and a vertical field of view — all `f64`.

use forge_frames::{DQuat, DVec3, FramePos};
use forge_num::det;

use crate::error::RenderError;

/// A perspective camera. Its axes (the **view axes**) are right-handed: `+x` right, `+y` up,
/// looking down `-z`. `orientation` maps view axes into the axes of `pos.frame`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Where the camera is.
    pub pos: FramePos,
    /// View axes -> the axes of `pos.frame`.
    pub orientation: DQuat,
    /// Vertical field of view, radians, in `(0, pi)`.
    pub fov_y: f64,
}

impl Camera {
    /// A camera at `pos` looking along `forward` with `up` roughly up (both in the axes of
    /// `pos.frame`). `None` if `forward` is zero or parallel to `up`, or `fov_y` is not in
    /// `(0, pi)`.
    #[must_use]
    pub fn looking(pos: FramePos, forward: DVec3, up: DVec3, fov_y: f64) -> Option<Self> {
        let f = forward.try_normalize()?;
        let x = f.cross(up).try_normalize()?;
        let z = -f;
        let y = z.cross(x);
        let cam = Self {
            pos,
            orientation: quat_from_basis(x, y, z),
            fov_y,
        };
        cam.validate().ok().map(|()| cam)
    }

    /// `tan(fov_y / 2)`.
    #[must_use]
    pub fn tan_half_fov(&self) -> f64 {
        let h = 0.5 * self.fov_y;
        det::sin(h) / det::cos(h)
    }

    pub(crate) fn validate(&self) -> Result<(), RenderError> {
        if !(self.fov_y > 1e-6 && self.fov_y < core::f64::consts::PI - 1e-6) {
            return Err(RenderError::scene(format!(
                "camera field of view {} rad is not in (0, pi)",
                self.fov_y
            )));
        }
        if !self.pos.local.is_finite() || !self.orientation.is_finite() {
            return Err(RenderError::scene(
                "camera position or orientation is not finite",
            ));
        }
        if (self.orientation.length_squared() - 1.0).abs() > 1e-6 {
            return Err(RenderError::scene(
                "camera orientation is not a unit quaternion",
            ));
        }
        Ok(())
    }
}

/// The rotation whose columns are the orthonormal right-handed basis `x, y, z`.
#[must_use]
pub fn quat_from_basis(x: DVec3, y: DVec3, z: DVec3) -> DQuat {
    let (m00, m01, m02) = (x.x, y.x, z.x);
    let (m10, m11, m12) = (x.y, y.y, z.y);
    let (m20, m21, m22) = (x.z, y.z, z.z);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = det::sqrt(trace + 1.0) * 2.0;
        DQuat::from_xyzw((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = det::sqrt(1.0 + m00 - m11 - m22) * 2.0;
        DQuat::from_xyzw(0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = det::sqrt(1.0 + m11 - m00 - m22) * 2.0;
        DQuat::from_xyzw((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = det::sqrt(1.0 + m22 - m00 - m11) * 2.0;
        DQuat::from_xyzw((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    };
    q.try_normalize().unwrap_or(DQuat::IDENTITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::FrameId;

    #[test]
    fn looking_down_minus_z_is_identity_and_axes_map() {
        let p = FramePos::origin_of(FrameId(0));
        let c = Camera::looking(p, DVec3::new(0.0, 0.0, -1.0), DVec3::Y, 1.0).expect("cam");
        let d = c.orientation.rotate(DVec3::new(0.0, 0.0, -1.0));
        assert!((d - DVec3::new(0.0, 0.0, -1.0)).length() < 1e-12);
        let c = Camera::looking(p, DVec3::X, DVec3::Z, 1.0).expect("cam");
        // View -z maps to +x (forward), view +y maps to +z (up).
        assert!((c.orientation.rotate(DVec3::new(0.0, 0.0, -1.0)) - DVec3::X).length() < 1e-12);
        assert!((c.orientation.rotate(DVec3::Y) - DVec3::Z).length() < 1e-12);
        assert!(Camera::looking(p, DVec3::Z, DVec3::Z, 1.0).is_none());
        assert!(Camera::looking(p, DVec3::X, DVec3::Z, 4.0).is_none());
    }
}
