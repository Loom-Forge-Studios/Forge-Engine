//! **Instance placement.** Where an instance is put decides where its content is.
//!
//! The editor's transforms are flat (an entity's `Transform` is a frame and an offset, not
//! relative to its parent), so a scene's layout is read **relative to its root**: an
//! instance's root pose, against the scene root's pose, is a rigid placement (translation,
//! rotation, uniform scale, and the instance's frame) applied to every mirrored node's
//! transform. Move or turn an instance and its whole content moves and turns with it; move a
//! node inside the scene and every instance's copy moves by the same amount, where it
//! stands.
//!
//! A mirror's **expected** content is its base's content placed this way; an override is
//! a difference from the expected value, so moving an instance does not count as
//! overriding the transforms of its nodes.
//!
//! The arithmetic is IEEE `+ - * /` and forge-num's bit-reproducible `sin`/`cos`/`atan2`
//! (Ch.3): the placed values are identical on every platform, so an instance saved on one
//! machine expands to the same bits on another. An instance at its scene root's pose places
//! nothing: its mirrors hold their bases' values exactly.

use std::borrow::Cow;
use std::collections::BTreeMap;

use forge_cmd::Value;
use forge_num::{DQuat, DVec3, det};

/// The `Transform` component's paths (`forge_editor::viewport::scene`; the editor checks
/// they agree).
pub const P_FRAME: &str = "transform.position.frame";
/// The position in its frame (`Vec3`, metres).
pub const P_LOCAL: &str = "transform.position.local";
/// Yaw about Y, degrees.
pub const P_YAW: &str = "transform.yaw";
/// Pitch about X, degrees.
pub const P_PITCH: &str = "transform.pitch";
/// Roll about Z, degrees.
pub const P_ROLL: &str = "transform.roll";
/// Uniform scale.
pub const P_SCALE: &str = "transform.scale";

/// A node's content (non-bookkeeping properties by path).
pub type Content = BTreeMap<String, Value>;

fn float(c: &Content, k: &str) -> Option<f64> {
    match c.get(k) {
        Some(Value::Float(v)) => Some(*v),
        Some(Value::Int(v)) => Some(*v as f64),
        _ => None,
    }
}

fn frame(c: &Content) -> i64 {
    match c.get(P_FRAME) {
        Some(Value::Int(f)) => *f,
        _ => 0,
    }
}

fn local(c: &Content) -> Option<DVec3> {
    match c.get(P_LOCAL) {
        Some(Value::Vec3(v)) => Some(DVec3::new(v[0], v[1], v[2])),
        _ => None,
    }
}

fn euler(c: &Content) -> (f64, f64, f64) {
    (
        float(c, P_YAW).unwrap_or(0.0),
        float(c, P_PITCH).unwrap_or(0.0),
        float(c, P_ROLL).unwrap_or(0.0),
    )
}

const DEG: f64 = core::f64::consts::PI / 180.0;

/// Euler angles in degrees (yaw about Y, pitch about X, roll about Z, applied yaw → pitch →
/// roll: `q = q_yaw · q_pitch · q_roll`), the editor's convention
/// (`forge_ui::widgets::euler_to_quat`), with deterministic trig.
#[must_use]
pub fn euler_to_quat(yaw: f64, pitch: f64, roll: f64) -> DQuat {
    let h = |a: f64| (det::sin(a * DEG * 0.5), det::cos(a * DEG * 0.5));
    let (sy, cy) = h(yaw);
    let (sp, cp) = h(pitch);
    let (sr, cr) = h(roll);
    DQuat::from_xyzw(0.0, sy, 0.0, cy)
        * DQuat::from_xyzw(sp, 0.0, 0.0, cp)
        * DQuat::from_xyzw(0.0, 0.0, sr, cr)
}

/// The inverse of [`euler_to_quat`] (degrees), the editor's convention, deterministic.
#[must_use]
pub fn quat_to_euler(q: DQuat) -> (f64, f64, f64) {
    let n = det::sqrt(q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w);
    let (x, y, z, w) = if n > 0.0 {
        (q.x / n, q.y / n, q.z / n, q.w / n)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    };
    let r00 = 1.0 - 2.0 * (y * y + z * z);
    let r02 = 2.0 * (x * z + y * w);
    let r10 = 2.0 * (x * y + z * w);
    let r11 = 1.0 - 2.0 * (x * x + z * z);
    let r12 = 2.0 * (y * z - x * w);
    let r20 = 2.0 * (x * z - y * w);
    let r22 = 1.0 - 2.0 * (x * x + y * y);
    let sp = (-r12).clamp(-1.0, 1.0);
    let pitch = det::atan2(sp, det::sqrt(1.0 - sp * sp));
    let (yaw, roll) = if sp.abs() > 0.999_999 {
        (det::atan2(-r20, r00), 0.0)
    } else {
        (det::atan2(r02, r22), det::atan2(r10, r11))
    };
    (yaw / DEG, pitch / DEG, roll / DEG)
}

/// How an instance places its scene's nodes (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// The scene root's frame and position; the instance root's frame and position.
    from: (i64, DVec3),
    to: (i64, DVec3),
    /// Rotation from the scene root's axes to the instance root's, if they differ.
    turn: Option<DQuat>,
    /// Scale ratio, if not 1.
    scale: Option<f64>,
    identity: bool,
}

impl Placement {
    /// The placement of an instance whose root holds `instance` against a scene root
    /// holding `scene`.
    #[must_use]
    pub fn between(instance: &Content, scene: &Content) -> Self {
        let same = |k: &str| instance.get(k) == scene.get(k);
        let identity = [P_FRAME, P_LOCAL, P_YAW, P_PITCH, P_ROLL, P_SCALE]
            .iter()
            .all(|k| same(k));
        let from = (frame(scene), local(scene).unwrap_or(DVec3::ZERO));
        let to = (frame(instance), local(instance).unwrap_or(DVec3::ZERO));
        let (ei, es) = (euler(instance), euler(scene));
        let turn = (ei != es).then(|| {
            let qi = euler_to_quat(ei.0, ei.1, ei.2);
            let qs = euler_to_quat(es.0, es.1, es.2);
            qi * qs.conjugate()
        });
        let (si, ss) = (
            float(instance, P_SCALE).unwrap_or(1.0),
            float(scene, P_SCALE).unwrap_or(1.0),
        );
        let scale = (si != ss && ss != 0.0).then(|| si / ss);
        Self {
            from,
            to,
            turn,
            scale,
            identity,
        }
    }

    /// Does it leave every value as it is?
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.identity
    }

    /// `base`'s content placed (see the module docs). Only transform paths change, and only
    /// for a node in the scene root's frame.
    #[must_use]
    pub fn apply<'a>(&self, base: &'a Content) -> Cow<'a, Content> {
        if self.identity {
            return Cow::Borrowed(base);
        }
        // Only a node with a position is placed (a node without a transform has no place),
        // and only in the scene root's frame (another frame does not compose with it).
        let Some(p) = local(base) else {
            return Cow::Borrowed(base);
        };
        if frame(base) != self.from.0 {
            return Cow::Borrowed(base);
        }
        let mut out = base.clone();
        if self.to.0 != self.from.0 || base.contains_key(P_FRAME) {
            out.insert(P_FRAME.to_string(), Value::Int(self.to.0));
        }
        let d = p - self.from.1;
        let d = match self.turn {
            Some(q) => q.rotate(d),
            None => d,
        };
        let d = match self.scale {
            Some(s) => d * s,
            None => d,
        };
        let p2 = self.to.1 + d;
        out.insert(P_LOCAL.to_string(), Value::Vec3([p2.x, p2.y, p2.z]));
        if let Some(q) = self.turn {
            let (y, p, r) = euler(base);
            let (y2, p2, r2) = quat_to_euler(q * euler_to_quat(y, p, r));
            out.insert(P_YAW.to_string(), Value::Float(y2));
            out.insert(P_PITCH.to_string(), Value::Float(p2));
            out.insert(P_ROLL.to_string(), Value::Float(r2));
        }
        if let Some(s) = self.scale {
            let b = float(base, P_SCALE).unwrap_or(1.0);
            out.insert(P_SCALE.to_string(), Value::Float(b * s));
        }
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(pos: [f64; 3], yaw: f64, scale: f64) -> Content {
        let mut c = Content::new();
        c.insert(P_LOCAL.into(), Value::Vec3(pos));
        c.insert(P_YAW.into(), Value::Float(yaw));
        c.insert(P_SCALE.into(), Value::Float(scale));
        c
    }

    #[test]
    fn a_placement_moves_turns_and_scales_the_layout_rigidly() {
        let scene = pose([0.0, 0.0, 0.0], 0.0, 1.0);
        let inst = pose([10.0, 0.0, 0.0], 90.0, 2.0);
        let pl = Placement::between(&inst, &scene);
        let mut child = Content::new();
        child.insert(P_LOCAL.into(), Value::Vec3([0.0, 0.0, 1.0]));
        let out = pl.apply(&child).into_owned();
        let Some(Value::Vec3(p)) = out.get(P_LOCAL) else {
            panic!("placed")
        };
        // Yaw 90° about +Y turns +Z into +X; scaled by 2 and moved by +10 in X.
        assert!(
            (p[0] - 12.0).abs() < 1e-12 && p[1].abs() < 1e-12 && p[2].abs() < 1e-12,
            "{p:?}"
        );
        assert!(matches!(out.get(P_YAW), Some(Value::Float(y)) if (y - 90.0).abs() < 1e-9));
        assert_eq!(out.get(P_SCALE), Some(&Value::Float(2.0)));
    }

    #[test]
    fn an_instance_at_its_scene_roots_pose_places_nothing_exactly() {
        let scene = pose([1.25, -3.0, 0.1], 33.0, 1.5);
        let pl = Placement::between(&scene.clone(), &scene);
        assert!(pl.is_identity());
        let child = pose([0.1, 0.2, 0.3], 12.5, 0.7);
        assert_eq!(*pl.apply(&child), child);
        // A pure move is exact for the translation it adds.
        let mut moved = scene.clone();
        moved.insert(P_LOCAL.into(), Value::Vec3([2.25, -3.0, 0.1]));
        let out = Placement::between(&moved, &scene)
            .apply(&child)
            .into_owned();
        assert_eq!(out.get(P_YAW), child.get(P_YAW), "no turn written");
        assert_eq!(out.get(P_SCALE), child.get(P_SCALE), "no scale written");
    }

    #[test]
    fn euler_round_trips_in_the_editors_convention() {
        for (y, p, r) in [(10.0, 20.0, 30.0), (-120.0, 45.0, 170.0), (0.0, -60.0, 5.0)] {
            let (y2, p2, r2) = quat_to_euler(euler_to_quat(y, p, r));
            assert!((y - y2).abs() < 1e-9 && (p - p2).abs() < 1e-9 && (r - r2).abs() < 1e-9);
        }
    }
}
