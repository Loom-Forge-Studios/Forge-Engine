//! Transform gizmos (Ch.21 §21.21): translate, rotate and scale handles in **local**,
//! **world** or **frame** space, with snapping. The gizmo is pure geometry over the
//! camera-relative scene; what a drag changes becomes `SetProperty` commands on the
//! entity's `Transform`, sent by the transform tool inside one gesture (one transaction,
//! one undo entry; Esc cancels — Ch.21 §21.18).
//!
//! **Spaces.** *Local* is the entity's own axes. *Frame* is the axes of the frame its
//! position is expressed in (a moving platform's frame for a crate on it). *World* is
//! the axes of the root of the frame tree. With a rotating or tilted frame, World and Frame
//! differ — that is the point of offering both.
//!
//! **Scale.** The built-in `Transform` has one uniform scale, so every scale handle scales
//! uniformly (an axis handle measures the factor along its axis). A per-axis scale arrives
//! with a component that has one.

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_frames::{DQuat, DVec3, FramePos, FrameResolver, Tick};
use forge_ui::widgets::quat_to_euler;

use crate::viewport::camera::{EditorCamera, Projected, rotation_to_root};
use crate::viewport::scene::{
    Drawable, LineSink, LineStyle, P_LOCAL, P_PITCH, P_ROLL, P_SCALE, P_YAW,
};

/// On-screen length of a gizmo axis, in pixels (constant at any distance).
pub const GIZMO_PX: f64 = 96.0;
/// How close (pixels) the pointer must be to a handle to grab it.
pub const HIT_PX: f64 = 8.0;
/// The scale range the `Transform` component declares (`#[forge(min, max)]`).
pub const SCALE_RANGE: (f64, f64) = (0.001, 1000.0);

/// Which transform the gizmo edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoMode {
    #[default]
    Translate,
    Rotate,
    Scale,
}

impl GizmoMode {
    pub const ALL: [GizmoMode; 3] = [GizmoMode::Translate, GizmoMode::Rotate, GizmoMode::Scale];
    pub fn label(self) -> &'static str {
        match self {
            GizmoMode::Translate => forge_ui::tr!("Move"),
            GizmoMode::Rotate => forge_ui::tr!("Rotate"),
            GizmoMode::Scale => forge_ui::tr!("Scale"),
        }
    }
}

/// Which axes the gizmo's handles follow (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoSpace {
    Local,
    #[default]
    World,
    Frame,
}

impl GizmoSpace {
    pub const ALL: [GizmoSpace; 3] = [GizmoSpace::Local, GizmoSpace::World, GizmoSpace::Frame];
    pub fn label(self) -> &'static str {
        match self {
            GizmoSpace::Local => forge_ui::tr!("Local"),
            GizmoSpace::World => forge_ui::tr!("World"),
            GizmoSpace::Frame => forge_ui::tr!("Frame"),
        }
    }
}

/// Snapping (session state: a viewport toolbar setting).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub enabled: bool,
    /// Metres per translation step.
    pub translate: f64,
    /// Degrees per rotation step.
    pub rotate_deg: f64,
    /// Scale factor step.
    pub scale: f64,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            enabled: false,
            translate: 0.25,
            rotate_deg: 15.0,
            scale: 0.1,
        }
    }
}

fn snap_to(v: f64, step: f64, on: bool) -> f64 {
    if on && step > 0.0 {
        (v / step).round() * step
    } else {
        v
    }
}

/// A grabbable part of the gizmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Along axis 0..=2.
    Axis(u8),
    /// In the plane whose normal is axis 0..=2.
    Plane(u8),
    /// The ring about axis 0..=2.
    Ring(u8),
    /// The centre: move in the view plane, or scale uniformly.
    Centre,
}

/// The gizmo placed for a selection, in camera-relative terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoFrame {
    /// The pivot's offset from the camera (camera-frame axes).
    pub rel: DVec3,
    /// The three handle axes (camera-frame axes, unit).
    pub axes: [DVec3; 3],
    /// Handle length in metres at the pivot's depth.
    pub len: f64,
}

/// The pivot's frame axes → camera-frame axes, and the gizmo's axes in the chosen space.
pub fn place(
    cam: &EditorCamera,
    tree: &dyn FrameResolver,
    t: Tick,
    pivot: &Drawable,
    space: GizmoSpace,
    height: f64,
) -> Option<GizmoFrame> {
    let rel = cam.offset_of(tree, t, pivot.pos)?;
    let to_cam = cam.rotation_from(tree, t, pivot.pos.frame)?;
    let axis_in_frame = |e: DVec3| -> Option<DVec3> {
        Some(match space {
            GizmoSpace::Local => pivot.rotation.rotate(e),
            GizmoSpace::Frame => e,
            GizmoSpace::World => {
                let (f_to_root, _) = rotation_to_root(tree, pivot.pos.frame, t)?;
                f_to_root.inverse_rotate(e)
            }
        })
    };
    let mut axes = [DVec3::X, DVec3::Y, DVec3::Z];
    for a in &mut axes {
        *a = to_cam.rotate(axis_in_frame(*a)?).try_normalize()?;
    }
    let depth = -cam.orientation.inverse_rotate(rel).z;
    if depth <= 0.0 {
        return None;
    }
    Some(GizmoFrame {
        rel,
        axes,
        len: cam.metres_per_pixel(depth, height) * GIZMO_PX,
    })
}

fn dist_to_segment(p: (f64, f64), a: &Projected, b: &Projected) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let s = if l2 > 0.0 {
        (((p.0 - a.x) * dx + (p.1 - a.y) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.x + dx * s - p.0, a.y + dy * s - p.1);
    (qx * qx + qy * qy).sqrt()
}

fn in_quad(p: (f64, f64), q: &[Projected; 4]) -> bool {
    let mut inside = false;
    let mut j = 3;
    for i in 0..4 {
        let (a, b) = (&q[i], &q[j]);
        if (a.y > p.1) != (b.y > p.1) && p.0 < (b.x - a.x) * (p.1 - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The handle under pixel `p` of a `w x h` viewport, if any (nearest wins).
pub fn hit(
    cam: &EditorCamera,
    g: &GizmoFrame,
    mode: GizmoMode,
    p: (f64, f64),
    w: f64,
    h: f64,
) -> Option<Handle> {
    let pr = |v: DVec3| cam.project(v, w, h);
    let centre = pr(g.rel)?;
    if ((centre.x - p.0).powi(2) + (centre.y - p.1).powi(2)).sqrt() <= HIT_PX * 1.25
        && mode != GizmoMode::Rotate
    {
        return Some(Handle::Centre);
    }
    let mut best: Option<(f64, Handle)> = None;
    let mut consider = |d: f64, hnd: Handle| {
        if d <= HIT_PX && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, hnd));
        }
    };
    match mode {
        GizmoMode::Translate | GizmoMode::Scale => {
            for (i, a) in g.axes.iter().enumerate() {
                if let Some(tip) = pr(g.rel + *a * g.len) {
                    consider(dist_to_segment(p, &centre, &tip), Handle::Axis(i as u8));
                }
            }
            if mode == GizmoMode::Translate {
                for n in 0..3u8 {
                    let (i, j) = ((n as usize + 1) % 3, (n as usize + 2) % 3);
                    let (u, v) = (g.axes[i] * g.len, g.axes[j] * g.len);
                    let c = [
                        g.rel + u * 0.25 + v * 0.25,
                        g.rel + u * 0.45 + v * 0.25,
                        g.rel + u * 0.45 + v * 0.45,
                        g.rel + u * 0.25 + v * 0.45,
                    ];
                    if let (Some(a), Some(b), Some(cc), Some(d)) =
                        (pr(c[0]), pr(c[1]), pr(c[2]), pr(c[3]))
                        && in_quad(p, &[a, b, cc, d])
                    {
                        consider(0.0, Handle::Plane(n));
                    }
                }
            }
        }
        GizmoMode::Rotate => {
            for (n, a) in g.axes.iter().enumerate() {
                let i = (n + 1) % 3;
                let (u, v) = (g.axes[i], a.cross(g.axes[i]));
                const N: usize = 48;
                let at = |k: usize| {
                    let ang = std::f64::consts::TAU * k as f64 / N as f64;
                    pr(g.rel + (u * ang.cos() + v * ang.sin()) * g.len)
                };
                for k in 0..N {
                    if let (Some(a), Some(b)) = (at(k), at(k + 1)) {
                        consider(dist_to_segment(p, &a, &b), Handle::Ring(n as u8));
                    }
                }
            }
        }
    }
    best.map(|(_, h)| h)
}

/// Draw the gizmo (`hot`: the hovered or dragged handle).
pub fn draw(sink: &mut LineSink<'_>, g: &GizmoFrame, mode: GizmoMode, hot: Option<Handle>) {
    let is_hot = |h: Handle| hot == Some(h);
    match mode {
        GizmoMode::Translate | GizmoMode::Scale => {
            for (i, a) in g.axes.iter().enumerate() {
                let hnd = Handle::Axis(i as u8);
                let style = LineStyle::Gizmo {
                    axis: i as u8,
                    hot: is_hot(hnd),
                };
                let tip = g.rel + *a * g.len;
                sink.seg(g.rel, tip, style);
                if mode == GizmoMode::Scale {
                    // A small cube at the tip.
                    sink.boxed(tip, DQuat::IDENTITY, DVec3::splat(g.len * 0.05), style);
                } else {
                    // An arrowhead: two short strokes back from the tip.
                    let side = g.axes[(i + 1) % 3];
                    sink.seg(
                        tip,
                        tip - *a * (g.len * 0.15) + side * (g.len * 0.06),
                        style,
                    );
                    sink.seg(
                        tip,
                        tip - *a * (g.len * 0.15) - side * (g.len * 0.06),
                        style,
                    );
                }
            }
            if mode == GizmoMode::Translate {
                for n in 0..3u8 {
                    let (i, j) = ((n as usize + 1) % 3, (n as usize + 2) % 3);
                    let (u, v) = (g.axes[i] * g.len, g.axes[j] * g.len);
                    let c = [
                        g.rel + u * 0.25 + v * 0.25,
                        g.rel + u * 0.45 + v * 0.25,
                        g.rel + u * 0.45 + v * 0.45,
                        g.rel + u * 0.25 + v * 0.45,
                    ];
                    let style = LineStyle::Gizmo {
                        axis: n,
                        hot: is_hot(Handle::Plane(n)),
                    };
                    for k in 0..4 {
                        sink.seg(c[k], c[(k + 1) % 4], style);
                    }
                }
            }
            let s = g.len * 0.06;
            sink.boxed(
                g.rel,
                DQuat::IDENTITY,
                DVec3::splat(s),
                LineStyle::Gizmo {
                    axis: 3,
                    hot: is_hot(Handle::Centre),
                },
            );
        }
        GizmoMode::Rotate => {
            for (n, a) in g.axes.iter().enumerate() {
                sink.circle(
                    g.rel,
                    *a,
                    g.len,
                    LineStyle::Gizmo {
                        axis: n as u8,
                        hot: is_hot(Handle::Ring(n as u8)),
                    },
                );
            }
        }
    }
}

// ---- dragging -------------------------------------------------------------------------------

/// One selected entity's transform when the drag began.
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub key: EntityKey,
    pub pos: FramePos,
    pub rotation: DQuat,
    pub scale: f64,
    /// Its offset from the camera and its frame → camera-frame rotation, at the start.
    rel: DVec3,
    to_cam: DQuat,
}

/// A drag in progress (see the module docs). The camera does not move during a drag.
#[derive(Clone, Debug, PartialEq)]
pub struct GizmoDrag {
    pub handle: Handle,
    pub mode: GizmoMode,
    frame: GizmoFrame,
    starts: Vec<Start>,
    /// The pointer's value on the handle when grabbed (axis parameter, plane hit, angle
    /// vector or scale distance).
    grab: DVec3,
    grab_px: (f64, f64),
    /// The screen-space centre, for edge-on rings and centre scaling.
    centre_px: (f64, f64),
}

/// The value of the pointer ray on a handle's constraint: `x` is the axis parameter for an
/// axis, the full hit point (relative to the pivot) for a plane or ring.
fn constraint(
    cam: &EditorCamera,
    g: &GizmoFrame,
    handle: Handle,
    mode: GizmoMode,
    p: (f64, f64),
    size: (f64, f64),
) -> Option<DVec3> {
    let d = cam.ray_dir(p.0, p.1, size.0, size.1);
    let plane_hit = |n: DVec3| -> Option<DVec3> {
        let denom = d.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let s = g.rel.dot(n) / denom;
        (s > 0.0).then(|| d * s - g.rel)
    };
    match (handle, mode) {
        (Handle::Axis(i), _) => {
            // Closest point on the axis line c + A s to the ray D u (camera at the origin).
            let a = g.axes[i as usize];
            let (b, c0) = (a.dot(d), g.rel);
            let denom = 1.0 - b * b;
            if denom.abs() < 1e-9 {
                return None;
            }
            let s = (b * c0.dot(d) - c0.dot(a)) / denom;
            Some(DVec3::new(s, 0.0, 0.0))
        }
        (Handle::Plane(n), _) | (Handle::Ring(n), _) => plane_hit(g.axes[n as usize]),
        (Handle::Centre, GizmoMode::Translate) => plane_hit(cam.forward()),
        (Handle::Centre, _) => Some(DVec3::ZERO),
    }
}

impl GizmoDrag {
    /// Grab `handle` at pixel `p`. `selection` are the selected drawables (the first is the
    /// pivot); `None` if the handle is edge-on or behind the camera.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        cam: &EditorCamera,
        tree: &dyn FrameResolver,
        t: Tick,
        g: GizmoFrame,
        mode: GizmoMode,
        handle: Handle,
        selection: &[&Drawable],
        p: (f64, f64),
        size: (f64, f64),
    ) -> Option<Self> {
        let grab = constraint(cam, &g, handle, mode, p, size)?;
        let centre = cam.project(g.rel, size.0, size.1)?;
        let mut starts = Vec::with_capacity(selection.len());
        for d in selection {
            starts.push(Start {
                key: d.key,
                pos: d.pos,
                rotation: d.rotation,
                scale: d.scale,
                rel: cam.offset_of(tree, t, d.pos)?,
                to_cam: cam.rotation_from(tree, t, d.pos.frame)?,
            });
        }
        Some(Self {
            handle,
            mode,
            frame: g,
            starts,
            grab,
            grab_px: p,
            centre_px: (centre.x, centre.y),
        })
    }

    /// The entities being dragged.
    pub fn keys(&self) -> impl Iterator<Item = EntityKey> + '_ {
        self.starts.iter().map(|s| s.key)
    }

    /// The transforms for the pointer at pixel `p`: `(key, position, rotation, scale)` per
    /// entity. `None` while the pointer is off the constraint (edge-on); keep the last.
    pub fn update(
        &self,
        cam: &EditorCamera,
        p: (f64, f64),
        size: (f64, f64),
        snap: &Snap,
    ) -> Option<Vec<(EntityKey, FramePos, DQuat, f64)>> {
        let g = &self.frame;
        let now = constraint(cam, g, self.handle, self.mode, p, size);
        let out = match (self.mode, self.handle) {
            (GizmoMode::Translate, Handle::Axis(i)) => {
                let ds = snap_to(now?.x - self.grab.x, snap.translate, snap.enabled);
                self.translated(g.axes[i as usize] * ds)
            }
            (GizmoMode::Translate, Handle::Plane(n)) => {
                let delta = now? - self.grab;
                let (i, j) = ((n as usize + 1) % 3, (n as usize + 2) % 3);
                let di = snap_to(delta.dot(g.axes[i]), snap.translate, snap.enabled);
                let dj = snap_to(delta.dot(g.axes[j]), snap.translate, snap.enabled);
                self.translated(g.axes[i] * di + g.axes[j] * dj)
            }
            (GizmoMode::Translate, _) => {
                let delta = now? - self.grab;
                let r = cam.right();
                let u = cam.up();
                let dr = snap_to(delta.dot(r), snap.translate, snap.enabled);
                let du = snap_to(delta.dot(u), snap.translate, snap.enabled);
                self.translated(r * dr + u * du)
            }
            (GizmoMode::Rotate, Handle::Ring(n)) => {
                let a = g.axes[n as usize];
                let angle = match now {
                    Some(v) if self.grab.length_squared() > 0.0 => {
                        let v0 = self.grab;
                        v0.cross(v).dot(a).atan2(v0.dot(v))
                    }
                    _ => {
                        // Edge-on ring: turn by the pointer's angle about the screen centre,
                        // signed by which way the axis faces.
                        let ang =
                            |q: (f64, f64)| (q.1 - self.centre_px.1).atan2(q.0 - self.centre_px.0);
                        let facing = if cam.forward().dot(a) > 0.0 {
                            1.0
                        } else {
                            -1.0
                        };
                        (ang(p) - ang(self.grab_px)) * facing
                    }
                };
                let deg = snap_to(angle.to_degrees(), snap.rotate_deg, snap.enabled);
                self.rotated(a, deg.to_radians())
            }
            (GizmoMode::Scale, handle) => {
                let k = match handle {
                    Handle::Axis(_) => {
                        let s0 = self.grab.x;
                        if s0.abs() < 1e-12 {
                            return None;
                        }
                        now?.x / s0
                    }
                    _ => {
                        // Centre: drag right or up to grow, 100 px doubles.
                        let dx = p.0 - self.grab_px.0;
                        let dy = self.grab_px.1 - p.1;
                        2f64.powf((dx + dy) / 100.0)
                    }
                };
                let k = snap_to(k, snap.scale, snap.enabled).max(1e-6);
                self.scaled(k)
            }
            _ => return None,
        };
        Some(out)
    }

    /// Everyone moved by `delta` (camera-frame axes), each in its own frame.
    fn translated(&self, delta: DVec3) -> Vec<(EntityKey, FramePos, DQuat, f64)> {
        self.starts
            .iter()
            .map(|s| {
                let local = s.pos.local + s.to_cam.inverse_rotate(delta);
                (
                    s.key,
                    FramePos::new(s.pos.frame, local),
                    s.rotation,
                    s.scale,
                )
            })
            .collect()
    }

    /// Everyone turned by `angle` about `axis` (camera-frame) through the pivot.
    fn rotated(&self, axis: DVec3, angle: f64) -> Vec<(EntityKey, FramePos, DQuat, f64)> {
        let q = DQuat::from_axis_angle(axis, angle);
        let c = self.frame.rel;
        self.starts
            .iter()
            .map(|s| {
                let moved = c + q.rotate(s.rel - c);
                let local = s.pos.local + s.to_cam.inverse_rotate(moved - s.rel);
                let rot = (s.to_cam.conjugate() * q * s.to_cam * s.rotation)
                    .try_normalize()
                    .unwrap_or(s.rotation);
                (s.key, FramePos::new(s.pos.frame, local), rot, s.scale)
            })
            .collect()
    }

    /// Everyone scaled by `k` about the pivot (the scale clamped to the component's range).
    fn scaled(&self, k: f64) -> Vec<(EntityKey, FramePos, DQuat, f64)> {
        let c = self.frame.rel;
        self.starts
            .iter()
            .map(|s| {
                let scale = (s.scale * k).clamp(SCALE_RANGE.0, SCALE_RANGE.1);
                let k = scale / s.scale;
                let moved = c + (s.rel - c) * k;
                let local = s.pos.local + s.to_cam.inverse_rotate(moved - s.rel);
                (s.key, FramePos::new(s.pos.frame, local), s.rotation, scale)
            })
            .collect()
    }

    /// The `SetProperty` commands that take each entity from its start to `to` (only what
    /// changed; a `Transform` edit is its position, its yaw / pitch / roll, its scale).
    pub fn commands(&self, to: &[(EntityKey, FramePos, DQuat, f64)]) -> Vec<EditorCommand> {
        let mut out = Vec::new();
        for (s, (key, p, r, sc)) in self.starts.iter().zip(to) {
            if p.local != s.pos.local {
                out.push(EditorCommand::SetProperty {
                    entity: *key,
                    path: P_LOCAL.into(),
                    value: Value::Vec3([p.local.x, p.local.y, p.local.z]),
                });
            }
            if *r != s.rotation {
                let (yaw, pitch, roll) = quat_to_euler(*r);
                for (path, v) in [(P_YAW, yaw), (P_PITCH, pitch), (P_ROLL, roll)] {
                    out.push(EditorCommand::SetProperty {
                        entity: *key,
                        path: path.into(),
                        value: Value::Float(v),
                    });
                }
            }
            if *sc != s.scale {
                out.push(EditorCommand::SetProperty {
                    entity: *key,
                    path: P_SCALE.into(),
                    value: Value::Float(*sc),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::FrameId;
    use forge_frames::testing::FixedFrames;

    fn setup() -> (EditorCamera, FixedFrames, Drawable) {
        let tree = FixedFrames::new();
        let cam = EditorCamera::new(FrameId(0));
        let d = Drawable {
            key: EntityKey(1),
            name: "cube".into(),
            pos: FramePos::origin_of(FrameId(0)),
            rotation: DQuat::IDENTITY,
            scale: 1.0,
            seed_path: None,
            locked: false,
            layer: false,
        };
        (cam, tree, d)
    }

    const SIZE: (f64, f64) = (800.0, 600.0);

    fn tip_px(cam: &EditorCamera, g: &GizmoFrame, i: usize, frac: f64) -> (f64, f64) {
        let p = cam
            .project(g.rel + g.axes[i] * (g.len * frac), SIZE.0, SIZE.1)
            .expect("front");
        (p.x, p.y)
    }

    #[test]
    fn axis_handles_are_hit_along_their_length_and_missed_away() {
        let (cam, tree, d) = setup();
        let g = place(&cam, &tree, Tick(0), &d, GizmoSpace::World, SIZE.1).expect("placed");
        let at = tip_px(&cam, &g, 0, 0.8);
        assert_eq!(
            hit(&cam, &g, GizmoMode::Translate, at, SIZE.0, SIZE.1),
            Some(Handle::Axis(0))
        );
        let far = (at.0, at.1 - 60.0);
        assert_eq!(hit(&cam, &g, GizmoMode::Scale, far, SIZE.0, SIZE.1), None);
    }

    #[test]
    fn dragging_the_x_axis_moves_along_x_only_and_snaps() {
        let (cam, tree, d) = setup();
        let g = place(&cam, &tree, Tick(0), &d, GizmoSpace::World, SIZE.1).expect("placed");
        let from = tip_px(&cam, &g, 0, 0.5);
        let drag = GizmoDrag::begin(
            &cam,
            &tree,
            Tick(0),
            g,
            GizmoMode::Translate,
            Handle::Axis(0),
            &[&d],
            from,
            SIZE,
        )
        .expect("grab");
        let to = tip_px(&cam, &g, 0, 3.2);
        let out = drag
            .update(&cam, to, SIZE, &Snap::default())
            .expect("moved");
        let p = out[0].1.local;
        assert!(p.x > 0.1 && p.y.abs() < 1e-9 && p.z.abs() < 1e-9, "{p:?}");
        let snap = Snap {
            enabled: true,
            translate: 0.25,
            ..Snap::default()
        };
        let out = drag.update(&cam, to, SIZE, &snap).expect("moved");
        let x = out[0].1.local.x;
        assert!(((x / 0.25).round() * 0.25 - x).abs() < 1e-9, "{x}");
        let cmds = drag.commands(&out);
        assert_eq!(cmds.len(), 1, "{cmds:?}");
    }

    #[test]
    fn world_and_frame_space_differ_in_a_turned_frame() {
        let (cam, mut tree, mut d) = setup();
        // A frame turned 90° about y.
        let f = tree.add(
            DVec3::ZERO,
            DQuat::from_axis_angle(DVec3::Y, std::f64::consts::FRAC_PI_2),
        );
        d.pos = FramePos::origin_of(f);
        let world = place(&cam, &tree, Tick(0), &d, GizmoSpace::World, SIZE.1).expect("w");
        let frame = place(&cam, &tree, Tick(0), &d, GizmoSpace::Frame, SIZE.1).expect("f");
        // Camera frame is the root: World's x axis is the root's x.
        assert!(
            (world.axes[0] - DVec3::X).length() < 1e-9,
            "{:?}",
            world.axes[0]
        );
        // The frame's x axis is the root's -z after a 90° turn about y.
        assert!(
            (frame.axes[0] - DVec3::new(0.0, 0.0, -1.0)).length() < 1e-9,
            "{:?}",
            frame.axes[0]
        );
        // Moving along World x changes the entity's frame-local z.
        let from = tip_px(&cam, &world, 0, 0.5);
        let to = tip_px(&cam, &world, 0, 2.0);
        let drag = GizmoDrag::begin(
            &cam,
            &tree,
            Tick(0),
            world,
            GizmoMode::Translate,
            Handle::Axis(0),
            &[&d],
            from,
            SIZE,
        )
        .expect("grab");
        let out = drag
            .update(&cam, to, SIZE, &Snap::default())
            .expect("moved");
        let l = out[0].1.local;
        assert!(l.x.abs() < 1e-9 && l.z.abs() > 0.1, "{l:?}");
    }

    #[test]
    fn a_ring_drag_turns_about_its_axis_and_snaps_to_degrees() {
        let (cam, tree, d) = setup();
        let g = place(&cam, &tree, Tick(0), &d, GizmoSpace::World, SIZE.1).expect("placed");
        // Grab the y ring (horizontal circle) at its +x point, drag toward +z.
        let p0 = cam
            .project(g.rel + g.axes[0] * g.len, SIZE.0, SIZE.1)
            .expect("p0");
        let p1 = cam
            .project(
                g.rel + (g.axes[0] + g.axes[2]).try_normalize().expect("n") * g.len,
                SIZE.0,
                SIZE.1,
            )
            .expect("p1");
        let drag = GizmoDrag::begin(
            &cam,
            &tree,
            Tick(0),
            g,
            GizmoMode::Rotate,
            Handle::Ring(1),
            &[&d],
            (p0.x, p0.y),
            SIZE,
        )
        .expect("grab");
        let snap = Snap {
            enabled: true,
            rotate_deg: 15.0,
            ..Snap::default()
        };
        let out = drag
            .update(&cam, (p1.x, p1.y), SIZE, &snap)
            .expect("turned");
        let (yaw, pitch, roll) = quat_to_euler(out[0].2);
        assert!(
            (yaw.abs() - 45.0).abs() < 1e-6 && pitch.abs() < 1e-6 && roll.abs() < 1e-6,
            "{yaw} {pitch} {roll}"
        );
        let cmds = drag.commands(&out);
        assert_eq!(cmds.len(), 3, "yaw, pitch, roll: {cmds:?}");
    }

    #[test]
    fn scale_is_uniform_and_clamped_to_the_component_range() {
        let (cam, tree, d) = setup();
        let g = place(&cam, &tree, Tick(0), &d, GizmoSpace::Local, SIZE.1).expect("placed");
        let from = tip_px(&cam, &g, 1, 1.0);
        let to = tip_px(&cam, &g, 1, 2.0);
        let drag = GizmoDrag::begin(
            &cam,
            &tree,
            Tick(0),
            g,
            GizmoMode::Scale,
            Handle::Axis(1),
            &[&d],
            from,
            SIZE,
        )
        .expect("grab");
        let out = drag
            .update(&cam, to, SIZE, &Snap::default())
            .expect("scaled");
        assert!((out[0].3 - 2.0).abs() < 1e-6, "{}", out[0].3);
        let huge = drag.scaled(1.0e9);
        assert_eq!(huge[0].3, SCALE_RANGE.1);
    }
}
