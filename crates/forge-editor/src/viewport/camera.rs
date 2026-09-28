//! The editor camera (Ch.21 §21.21 "Viewport", Ch.2 §2.4): an `f64` [`FramePos`] and
//! orientation, orbit / fly / pan navigation, and **log-scaled** speed so one camera is
//! usable from a centimetre to the far end of a large world, and as far as its
//! [`EditorCamera::max_distance`] allows.
//!
//! **Camera-relative, always.** Nothing here ever forms an absolute position in `f32`.
//! Every point the viewport draws or picks is resolved into the camera's frame and the
//! camera's own position is subtracted **in `f64`** ([`EditorCamera::offset_of`]); only the
//! resulting small offset is projected, and only pixel coordinates are ever narrowed. That
//! is what keeps an object one metre in front of a camera thousands of kilometres from its
//! frame's origin steady on screen (`test_viewport_camera_precision`), where subtracting two `f32`
//! positions would move it by kilometres.
//!
//! The camera is **session state** (Ch.21 §21.18): moving it is never a command.

use forge_frames::{DQuat, DVec3, FrameId, FramePos, FrameResolver, Tick};

/// The nearest a camera orbits or frames (1 cm: where the depth pass starts, Ch.2.4).
pub const MIN_DISTANCE: f64 = 0.01;
/// The farthest a camera orbits, frames or dollies out by default: 1e7 m, the far end of a
/// single-frame world's one depth pass (Ch.2.4). A camera in a larger place carries its own
/// [`EditorCamera::max_distance`].
pub const MAX_DISTANCE: f64 = 1.0e7;
/// The narrowest and widest vertical field of view the viewport allows (radians).
pub const FOV_RANGE: (f64, f64) = (0.1, 2.6);
/// How far each wheel notch dollies: the distance is multiplied by this per notch in.
pub const DOLLY_PER_NOTCH: f64 = 0.85;
/// Fly-speed steps (each a factor of √2) the user can add or remove with the wheel.
pub const SPEED_STEPS: i32 = 24;
/// Radians of orbit / look per pixel dragged.
pub const RADIANS_PER_PIXEL: f64 = 0.005;
/// Pitch is kept this far (radians) from straight up or down, so "up" stays defined.
const PITCH_LIMIT: f64 = 1.553; // ~89°

/// What a primary drag on empty space does (the navigation mode on the viewport toolbar).
/// The secondary button always looks around (fly), the middle button always pans, the
/// wheel always dollies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NavMode {
    /// Turn around the pivot.
    #[default]
    Orbit,
    /// Look around from where the camera is (and WASD/QE moves it).
    Fly,
    /// Slide the view sideways.
    Pan,
}

impl NavMode {
    pub const ALL: [NavMode; 3] = [NavMode::Orbit, NavMode::Fly, NavMode::Pan];
    pub fn label(self) -> &'static str {
        match self {
            NavMode::Orbit => forge_ui::tr!("Orbit"),
            NavMode::Fly => forge_ui::tr!("Fly"),
            NavMode::Pan => forge_ui::tr!("Pan"),
        }
    }
}

/// Keyboard fly input for one step: each axis in `-1..=1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlyInput {
    /// `+1` forward (W), `-1` back (S).
    pub forward: f64,
    /// `+1` right (D), `-1` left (A).
    pub right: f64,
    /// `+1` up (E), `-1` down (Q).
    pub up: f64,
    /// Shift held: four times faster.
    pub boost: bool,
}

impl FlyInput {
    pub fn is_idle(&self) -> bool {
        self.forward == 0.0 && self.right == 0.0 && self.up == 0.0
    }
}

/// A point projected onto the viewport: pixels from the viewport's top-left, and the
/// distance in front of the camera (metres, along the view axis).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projected {
    pub x: f64,
    pub y: f64,
    pub depth: f64,
}

/// The editor camera (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorCamera {
    /// Where the camera is.
    pub pos: FramePos,
    /// View axes (`+x` right, `+y` up, looking down `-z`) → the axes of `pos.frame`, the
    /// same convention as `forge_render::Camera`.
    pub orientation: DQuat,
    /// Vertical field of view, radians.
    pub fov_y: f64,
    /// Distance from the camera to the pivot it orbits, along the view direction.
    pub pivot_distance: f64,
    /// The user's fly-speed adjustment, in √2 steps (`0`: the automatic speed).
    pub speed_steps: i32,
    /// The farthest this camera orbits, frames or dollies out ([`MAX_DISTANCE`] unless the
    /// place it is in is larger).
    pub max_distance: f64,
}

impl EditorCamera {
    /// A camera in `frame` at `(0, 2, 6)` m looking at the frame's origin (a sensible
    /// first view of a new scene).
    pub fn new(frame: FrameId) -> Self {
        let mut c = Self {
            pos: FramePos::new(frame, DVec3::new(0.0, 2.0, 6.0)),
            orientation: DQuat::IDENTITY,
            fov_y: 1.0,
            pivot_distance: 1.0,
            speed_steps: 0,
            max_distance: MAX_DISTANCE,
        };
        c.look_at(FramePos::origin_of(frame));
        c
    }

    /// The view direction in the axes of `pos.frame`.
    pub fn forward(&self) -> DVec3 {
        self.orientation.rotate(DVec3::new(0.0, 0.0, -1.0))
    }
    pub fn right(&self) -> DVec3 {
        self.orientation.rotate(DVec3::X)
    }
    pub fn up(&self) -> DVec3 {
        self.orientation.rotate(DVec3::Y)
    }

    /// The point the camera orbits (in the camera's frame).
    pub fn pivot(&self) -> FramePos {
        FramePos::new(
            self.pos.frame,
            self.pos.local + self.forward() * self.pivot_distance,
        )
    }

    /// Turn to look at `target` (in the camera's frame; up is the frame's `+y`) and make it
    /// the pivot. Nothing happens if `target` is where the camera is.
    pub fn look_at(&mut self, target: FramePos) {
        if target.frame != self.pos.frame {
            return;
        }
        let d = target.local - self.pos.local;
        let dist = d.length();
        if let Some(q) = look_rotation(d, DVec3::Y) {
            self.orientation = q;
            self.pivot_distance = dist.clamp(MIN_DISTANCE, self.max_distance);
        }
    }

    /// `tan(fov_y / 2)`.
    pub fn tan_half_fov(&self) -> f64 {
        (0.5 * self.fov_y).tan()
    }

    // ---- navigation ------------------------------------------------------------------------

    /// Orbit the pivot by a pointer drag of `(dx, dy)` pixels: yaw about the frame's up
    /// axis, pitch about the camera's right axis, keeping the pivot fixed.
    pub fn orbit(&mut self, dx: f64, dy: f64) {
        let pivot = self.pivot();
        self.rotate_view(-dx * RADIANS_PER_PIXEL, -dy * RADIANS_PER_PIXEL);
        self.pos.local = pivot.local - self.forward() * self.pivot_distance;
    }

    /// Look around from where the camera is (fly mode): the pivot moves with the view.
    pub fn look(&mut self, dx: f64, dy: f64) {
        self.rotate_view(-dx * RADIANS_PER_PIXEL, -dy * RADIANS_PER_PIXEL);
    }

    fn rotate_view(&mut self, yaw: f64, pitch: f64) {
        let fwd = self.forward();
        let cur_pitch = fwd.y.clamp(-1.0, 1.0).asin();
        let pitch = (cur_pitch + pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT) - cur_pitch;
        let q_yaw = DQuat::from_axis_angle(DVec3::Y, yaw);
        let q_pitch = DQuat::from_axis_angle(DVec3::X, pitch);
        // Yaw about the frame's up, pitch about the camera's own right axis.
        let q = (q_yaw * self.orientation * q_pitch).try_normalize();
        if let Some(q) = q {
            self.orientation = q;
        }
    }

    /// Slide the view by a drag of `(dx, dy)` pixels in a viewport `height` pixels tall: the
    /// point at the pivot's depth stays under the pointer.
    pub fn pan(&mut self, dx: f64, dy: f64, height: f64) {
        let per_px = 2.0 * self.pivot_distance * self.tan_half_fov() / height.max(1.0);
        let d = self.right() * (-dx * per_px) + self.up() * (dy * per_px);
        self.pos.local += d;
    }

    /// Dolly toward the pivot by `notches` wheel notches (positive: in). Logarithmic: each
    /// notch changes the distance by the same factor, so the wheel feels the same at 1 m and
    /// at 1e10 m. The pivot stays fixed.
    pub fn dolly(&mut self, notches: f64) {
        let pivot = self.pivot();
        let d = (self.pivot_distance * DOLLY_PER_NOTCH.powf(notches))
            .clamp(MIN_DISTANCE, self.max_distance);
        self.pivot_distance = d;
        self.pos.local = pivot.local - self.forward() * d;
    }

    /// The fly speed in m/s: the automatic speed (log-scaled with the distance to the
    /// pivot, so crossing the view takes about a second whether it spans 1 m or the whole world)
    /// times the user's √2 steps.
    pub fn fly_speed(&self) -> f64 {
        let auto = self.pivot_distance.clamp(MIN_DISTANCE, self.max_distance);
        let user = 2f64.powf(f64::from(self.speed_steps) * 0.5);
        (auto * user).clamp(0.05, self.max_distance)
    }

    /// Change the fly-speed steps by `delta` (the wheel while flying).
    pub fn adjust_speed(&mut self, delta: i32) {
        self.speed_steps = (self.speed_steps + delta).clamp(-SPEED_STEPS, SPEED_STEPS);
    }

    /// Move for `dt` seconds of keyboard fly input. The pivot keeps its distance, so the
    /// automatic speed does not run away while flying.
    pub fn fly(&mut self, input: FlyInput, dt: f64) {
        if input.is_idle() || dt <= 0.0 {
            return;
        }
        let speed = self.fly_speed() * if input.boost { 4.0 } else { 1.0 };
        let dir = self.forward() * input.forward + self.right() * input.right + DVec3::Y * input.up;
        if let Some(d) = dir.try_normalize() {
            self.pos.local += d * (speed * dt);
        }
    }

    /// Frame a sphere of `radius` metres around `target`: the camera moves into the
    /// target's frame (so precision is the target's own), keeps its view direction and
    /// backs off until the sphere fills most of the view.
    pub fn frame_sphere(
        &mut self,
        tree: &dyn FrameResolver,
        t: Tick,
        target: FramePos,
        radius: f64,
    ) {
        if target.frame != self.pos.frame {
            self.move_to_frame(tree, t, target.frame);
        }
        let r = radius.max(MIN_DISTANCE);
        let dist =
            (r / (0.5 * self.fov_y.min(1.2)).sin() * 1.1).clamp(MIN_DISTANCE, self.max_distance);
        self.pivot_distance = dist;
        self.pos = FramePos::new(target.frame, target.local - self.forward() * dist);
    }

    /// Re-express the camera in `frame` (its position resolved, its orientation turned into
    /// that frame's axes). Nothing changes if the frames are not connected.
    pub fn move_to_frame(&mut self, tree: &dyn FrameResolver, t: Tick, frame: FrameId) {
        let Ok(p) = tree.resolve(self.pos, frame, t) else {
            return;
        };
        let (Some((from_root, ra)), Some((to_root, rb))) = (
            rotation_to_root(tree, self.pos.frame, t),
            rotation_to_root(tree, frame, t),
        ) else {
            return;
        };
        if ra != rb {
            return;
        }
        // view -> old frame -> root -> new frame
        if let Some(q) = (to_root.conjugate() * from_root * self.orientation).try_normalize() {
            self.orientation = q;
            self.pos = p;
        }
    }

    // ---- projection (camera-relative, f64) ---------------------------------------------------

    /// Where `p` is relative to the camera, in the axes of the camera's frame: resolved into
    /// the camera's frame and the camera's position subtracted, both in `f64`. `None` if the
    /// frames are not connected.
    pub fn offset_of(&self, tree: &dyn FrameResolver, t: Tick, p: FramePos) -> Option<DVec3> {
        if p.frame == self.pos.frame {
            return Some(p.local - self.pos.local);
        }
        tree.resolve(p, self.pos.frame, t)
            .ok()
            .map(|q| q.local - self.pos.local)
    }

    /// The rotation from `frame`'s axes to the camera frame's axes. `None` if unconnected.
    pub fn rotation_from(
        &self,
        tree: &dyn FrameResolver,
        t: Tick,
        frame: FrameId,
    ) -> Option<DQuat> {
        if frame == self.pos.frame {
            return Some(DQuat::IDENTITY);
        }
        let (f, ra) = rotation_to_root(tree, frame, t)?;
        let (c, rb) = rotation_to_root(tree, self.pos.frame, t)?;
        (ra == rb).then(|| c.conjugate() * f)
    }

    /// Project a camera-relative offset (camera-frame axes) onto a `w x h` viewport.
    /// `None` behind the camera.
    pub fn project(&self, offset: DVec3, w: f64, h: f64) -> Option<Projected> {
        let v = self.orientation.inverse_rotate(offset);
        let depth = -v.z;
        if depth <= 1e-9 {
            return None;
        }
        let th = self.tan_half_fov();
        let aspect = w / h.max(1.0);
        let nx = v.x / (depth * th * aspect);
        let ny = v.y / (depth * th);
        Some(Projected {
            x: (nx + 1.0) * 0.5 * w,
            y: (1.0 - ny) * 0.5 * h,
            depth,
        })
    }

    /// The segment `a..b` (camera-relative offsets) clipped to the space in front of the
    /// camera and projected. `None` if it lies wholly behind.
    pub fn project_segment(
        &self,
        a: DVec3,
        b: DVec3,
        w: f64,
        h: f64,
    ) -> Option<(Projected, Projected)> {
        const NEAR: f64 = 1e-6;
        let va = self.orientation.inverse_rotate(a);
        let vb = self.orientation.inverse_rotate(b);
        let (da, db) = (-va.z, -vb.z);
        if da < NEAR && db < NEAR {
            return None;
        }
        let clip = |p: DVec3, q: DVec3, dp: f64, dq: f64| {
            let s = (NEAR - dp) / (dq - dp);
            p + (q - p) * s
        };
        let (a, b) = match (da < NEAR, db < NEAR) {
            (true, false) => (clip(a, b, da, db), b),
            (false, true) => (a, clip(b, a, db, da)),
            _ => (a, b),
        };
        Some((self.project(a, w, h)?, self.project(b, w, h)?))
    }

    /// The direction (camera-frame axes, unit) of the ray through pixel `(x, y)`.
    pub fn ray_dir(&self, x: f64, y: f64, w: f64, h: f64) -> DVec3 {
        let th = self.tan_half_fov();
        let aspect = w / h.max(1.0);
        let nx = (2.0 * x / w.max(1.0) - 1.0) * th * aspect;
        let ny = (1.0 - 2.0 * y / h.max(1.0)) * th;
        self.orientation
            .rotate(DVec3::new(nx, ny, -1.0))
            .try_normalize()
            .unwrap_or_else(|| self.forward())
    }

    /// Metres per pixel at `depth` in a viewport `height` pixels tall (gizmos keep a
    /// constant screen size with it).
    pub fn metres_per_pixel(&self, depth: f64, height: f64) -> f64 {
        2.0 * depth.max(MIN_DISTANCE) * self.tan_half_fov() / height.max(1.0)
    }
}

/// The rotation turning view axes so `-z` looks along `dir` with `up` roughly up.
pub fn look_rotation(dir: DVec3, up: DVec3) -> Option<DQuat> {
    let f = dir.try_normalize()?;
    let x = f
        .cross(up)
        .try_normalize()
        .or_else(|| f.cross(DVec3::Z).try_normalize())?;
    let z = -f;
    let y = z.cross(x);
    Some(quat_from_basis(x, y, z))
}

/// The rotation whose columns are the orthonormal right-handed basis `x, y, z`.
pub fn quat_from_basis(x: DVec3, y: DVec3, z: DVec3) -> DQuat {
    let (m00, m01, m02) = (x.x, y.x, z.x);
    let (m10, m11, m12) = (x.y, y.y, z.y);
    let (m20, m21, m22) = (x.z, y.z, z.z);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        DQuat::from_xyzw((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        DQuat::from_xyzw(0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        DQuat::from_xyzw((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        DQuat::from_xyzw((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    };
    q.try_normalize().unwrap_or(DQuat::IDENTITY)
}

/// `frame`'s axes → its root's axes at `t`, and the root (the resolver's answer). `None` for a
/// frame the resolver does not know.
pub fn rotation_to_root(
    tree: &dyn FrameResolver,
    frame: FrameId,
    t: Tick,
) -> Option<(DQuat, FrameId)> {
    tree.rotation_to_root(frame, t).ok()
}

/// W2 positive control for `test_viewport_camera_precision` only: project `p` the way a
/// camera **without** the camera-relative rule would — both positions narrowed to `f32`
/// first, then subtracted. **Never use.**
#[doc(hidden)]
#[cfg(any(test, feature = "controls"))]
pub fn project_naive_f32_for_control(
    cam: &EditorCamera,
    target: FramePos,
    w: f64,
    h: f64,
) -> Option<Projected> {
    let n = |v: f64| f64::from(v as f32);
    let off = DVec3::new(
        n(target.local.x) - n(cam.pos.local.x),
        n(target.local.y) - n(cam.pos.local.y),
        n(target.local.z) - n(cam.pos.local.z),
    );
    cam.project(off, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::WorldFrame;

    fn tree() -> WorldFrame {
        WorldFrame
    }

    #[test]
    fn new_camera_looks_at_the_origin_and_projects_it_to_the_centre() {
        let c = EditorCamera::new(FrameId(0));
        let tr = tree();
        let off = c
            .offset_of(&tr, Tick(0), FramePos::origin_of(FrameId(0)))
            .expect("offset");
        let p = c.project(off, 800.0, 600.0).expect("in front");
        assert!(
            (p.x - 400.0).abs() < 1e-9 && (p.y - 300.0).abs() < 1e-9,
            "{p:?}"
        );
        assert!((c.pivot_distance - 40f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn orbit_keeps_the_pivot_and_the_distance() {
        let mut c = EditorCamera::new(FrameId(0));
        let pivot = c.pivot();
        c.orbit(120.0, -40.0);
        let after = c.pivot();
        assert!((after.local - pivot.local).length() < 1e-9);
        assert!(((c.pos.local - pivot.local).length() - c.pivot_distance).abs() < 1e-9);
    }

    #[test]
    fn pitch_is_clamped_so_up_stays_defined() {
        let mut c = EditorCamera::new(FrameId(0));
        c.orbit(0.0, -100_000.0);
        assert!(c.forward().y.abs() < 0.9999, "{:?}", c.forward());
        c.orbit(0.0, 100_000.0);
        assert!(c.forward().y.abs() < 0.9999);
        assert!(c.orientation.is_finite());
    }

    #[test]
    fn dolly_is_logarithmic_and_clamped() {
        let mut c = EditorCamera::new(FrameId(0));
        c.pivot_distance = 1.0;
        c.dolly(1.0);
        assert!((c.pivot_distance - DOLLY_PER_NOTCH).abs() < 1e-12);
        c.pivot_distance = 1.0e6;
        c.dolly(1.0);
        assert!((c.pivot_distance - DOLLY_PER_NOTCH * 1.0e6).abs() < 1e-6);
        let mut far = c;
        far.max_distance = 1.0e22;
        far.dolly(-10_000.0);
        assert_eq!(
            far.pivot_distance, 1.0e22,
            "a camera in a larger place reaches farther"
        );
        c.dolly(-10_000.0);
        assert_eq!(c.pivot_distance, MAX_DISTANCE);
        c.dolly(10_000.0);
        assert_eq!(c.pivot_distance, MIN_DISTANCE);
    }

    #[test]
    fn fly_speed_scales_with_distance_on_a_log_scale() {
        let mut c = EditorCamera::new(FrameId(0));
        c.pivot_distance = 1.0;
        let near = c.fly_speed();
        c.pivot_distance = 1.0e6;
        let far = c.fly_speed();
        assert!((far / near - 1.0e6).abs() < 1.0, "{near} {far}");
        c.adjust_speed(2);
        assert!((c.fly_speed() / far - 2.0).abs() < 1e-12);
        c.adjust_speed(1000);
        assert_eq!(c.speed_steps, SPEED_STEPS);
    }

    #[test]
    fn pan_keeps_the_pivot_point_under_the_pointer() {
        let mut c = EditorCamera::new(FrameId(0));
        let tr = tree();
        let pivot = c.pivot();
        let (w, h) = (800.0, 600.0);
        c.pan(100.0, 50.0, h);
        let off = c.offset_of(&tr, Tick(0), pivot).expect("off");
        let p = c.project(off, w, h).expect("front");
        assert!(
            (p.x - 500.0).abs() < 1e-6 && (p.y - 350.0).abs() < 1e-6,
            "{p:?}"
        );
    }

    #[test]
    fn a_ray_through_a_projected_point_hits_it() {
        let mut c = EditorCamera::new(FrameId(0));
        c.orbit(33.0, 12.0);
        let off = DVec3::new(0.7, 0.2, -0.4) - c.pos.local;
        let p = c.project(off, 1024.0, 512.0).expect("front");
        let d = c.ray_dir(p.x, p.y, 1024.0, 512.0);
        let along = off.dot(d);
        assert!((off - d * along).length() < 1e-9);
    }

    #[test]
    fn segments_behind_are_clipped_not_flipped() {
        let c = EditorCamera::new(FrameId(0));
        let (w, h) = (800.0, 600.0);
        let behind = c.forward() * -5.0;
        let ahead = c.forward() * 5.0;
        assert!(c.project_segment(behind, behind * 2.0, w, h).is_none());
        let (a, b) = c.project_segment(behind, ahead, w, h).expect("clipped");
        assert!(a.depth > 0.0 && b.depth > 0.0);
    }
}
