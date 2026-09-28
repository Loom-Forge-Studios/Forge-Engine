//! `Skeleton2D` cutout rigging (Ch.35 §35.2).
//!
//! A cutout character is a tree of bones, each carrying sprite parts (a torso, an upper
//! arm, a hand...). A [`Clip`] keys bone rotations, translations and scales over time; a
//! [`Pose`] is sampled from a clip (or two clips blended), optionally corrected by
//! two-bone IK (a foot planted on uneven ground, a hand on a lever), and forward kinematics
//! ([`Skeleton2d::world`]) gives every bone's transform; [`Skeleton2d::sprites`] turns the
//! parts into sprites for the batcher.
//!
//! Angles go through `forge_num::det` (no platform `sin`/`cos`), time is whole microseconds:
//! a posed rig is bit-identical everywhere (I2 corpus row `2d/skeleton/*`). The editor's rig
//! panel (WP-U13) poses its bones through [`Skeleton2d::from_unordered`] and
//! [`Skeleton2d::world`].

use std::collections::BTreeMap;

use forge_frames::FramePos2;
use forge_num::{DVec2, det};

use crate::Error2d;
use crate::math::{Rot2, Xform2, angle_delta, lerp2};
use crate::sprite::{Sprite, TextureId, UvRect};

/// A bone's transform relative to its parent (radians, counter-clockwise).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneLocal {
    pub offset: DVec2,
    pub angle: f64,
    pub scale: DVec2,
}

impl Default for BoneLocal {
    fn default() -> Self {
        Self {
            offset: DVec2::ZERO,
            angle: 0.0,
            scale: DVec2::new(1.0, 1.0),
        }
    }
}

impl BoneLocal {
    fn xform(&self) -> Xform2 {
        Xform2 {
            translation: self.offset,
            rot: Rot2::from_angle(self.angle),
            scale: self.scale,
        }
    }
}

/// A bone.
#[derive(Clone, Debug, PartialEq)]
pub struct Bone2d {
    pub name: String,
    /// Always an earlier bone (parents come first).
    pub parent: Option<usize>,
    pub rest: BoneLocal,
    pub length: f64,
}

/// A skeleton: bones, parents before children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skeleton2d {
    pub bones: Vec<Bone2d>,
}

/// A pose: every bone's local transform.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    pub local: Vec<BoneLocal>,
}

/// A sprite part on a bone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub bone: usize,
    /// Size in world units and the pivot inside it (as [`Sprite::pivot`]).
    pub size: DVec2,
    pub pivot: DVec2,
    /// Placement relative to the bone.
    pub offset: DVec2,
    pub angle: f64,
    pub uv: UvRect,
    pub texture: TextureId,
    /// Draw order within the character.
    pub z: i32,
}

/// Keys of one bone in a clip; each list sorted by time (microseconds). Rotation keys are
/// added to the rest angle, translation keys to the rest offset, scale keys multiply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub bone: usize,
    pub rotation: Vec<(u64, f64)>,
    pub translation: Vec<(u64, DVec2)>,
    pub scale: Vec<(u64, DVec2)>,
}

/// An animation clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub name: String,
    pub duration_us: u64,
    pub looping: bool,
    pub tracks: Vec<Track>,
}

/// Linear key interpolation (holding the end values outside the keys).
fn sample_keys<T: Copy>(keys: &[(u64, T)], t: u64, mix: impl Fn(T, T, f64) -> T) -> Option<T> {
    let first = keys.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    for w in keys.windows(2) {
        let ((t0, a), (t1, b)) = (w[0], w[1]);
        if t < t1 {
            let span = (t1 - t0) as f64;
            let f = if span > 0.0 {
                (t - t0) as f64 / span
            } else {
                1.0
            };
            return Some(mix(a, b, f));
        }
    }
    keys.last().map(|k| k.1)
}

impl Skeleton2d {
    /// Check parents come first and every number is finite.
    pub fn validate(&self) -> Result<(), Error2d> {
        for (i, b) in self.bones.iter().enumerate() {
            if let Some(p) = b.parent
                && p >= i
            {
                return Err(Error2d::invalid(format!(
                    "bone {} ({}): its parent {p} does not come before it",
                    i, b.name
                )));
            }
            if !(b.rest.offset.is_finite()
                && b.rest.angle.is_finite()
                && b.rest.scale.is_finite()
                && b.length.is_finite())
            {
                return Err(Error2d::invalid(format!("bone {}: not finite", b.name)));
            }
        }
        Ok(())
    }

    /// Build from bones listed in any order by name, each naming its parent. Returns the
    /// skeleton (parents first), each named bone's index, and the bones that cannot be
    /// placed (a missing parent, or a loop), sorted.
    #[must_use]
    pub fn from_unordered(
        bones: &[(String, Option<String>, BoneLocal, f64)],
    ) -> (Skeleton2d, BTreeMap<String, usize>, Vec<String>) {
        let by_name: BTreeMap<&str, usize> = bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.0.as_str(), i))
            .collect();
        let mut placed: BTreeMap<String, usize> = BTreeMap::new();
        let mut sk = Skeleton2d::default();
        let mut broken = Vec::new();
        // Resolve each bone's chain to a root; order by depth then name for stability.
        let mut depth: BTreeMap<usize, usize> = BTreeMap::new();
        for (i, b) in bones.iter().enumerate() {
            let mut d = 0;
            let mut cur = b.1.as_deref();
            let mut ok = true;
            let mut seen = vec![i];
            while let Some(p) = cur {
                match by_name.get(p) {
                    Some(&pi) if !seen.contains(&pi) => {
                        seen.push(pi);
                        d += 1;
                        cur = bones[pi].1.as_deref();
                    }
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                depth.insert(i, d);
            } else {
                broken.push(b.0.clone());
            }
        }
        let mut order: Vec<usize> = depth.keys().copied().collect();
        order.sort_by(|a, b| depth[a].cmp(&depth[b]).then(bones[*a].0.cmp(&bones[*b].0)));
        for i in order {
            let (name, parent, rest, len) = &bones[i];
            let p = parent.as_ref().and_then(|p| placed.get(p).copied());
            sk.bones.push(Bone2d {
                name: name.clone(),
                parent: p,
                rest: *rest,
                length: *len,
            });
            placed.insert(name.clone(), sk.bones.len() - 1);
        }
        broken.sort();
        (sk, placed, broken)
    }

    /// The rest pose.
    #[must_use]
    pub fn rest_pose(&self) -> Pose {
        Pose {
            local: self.bones.iter().map(|b| b.rest).collect(),
        }
    }

    /// `clip` at `t_us` (wrapped when looping, held at the end otherwise).
    #[must_use]
    pub fn sample(&self, clip: &Clip, t_us: u64) -> Pose {
        let t = if clip.looping && clip.duration_us > 0 {
            t_us % clip.duration_us
        } else {
            t_us.min(clip.duration_us)
        };
        let mut pose = self.rest_pose();
        for tr in &clip.tracks {
            let Some(l) = pose.local.get_mut(tr.bone) else {
                continue;
            };
            if let Some(a) = sample_keys(&tr.rotation, t, |a, b, f| a + angle_delta(a, b) * f) {
                l.angle += a;
            }
            if let Some(o) = sample_keys(&tr.translation, t, lerp2) {
                l.offset += o;
            }
            if let Some(s) = sample_keys(&tr.scale, t, lerp2) {
                l.scale = l.scale.mul_elem(s);
            }
        }
        pose
    }

    /// `a` blended toward `b` by `w` (angles the short way).
    #[must_use]
    pub fn blend(a: &Pose, b: &Pose, w: f64) -> Pose {
        Pose {
            local: a
                .local
                .iter()
                .zip(&b.local)
                .map(|(x, y)| BoneLocal {
                    offset: lerp2(x.offset, y.offset, w),
                    angle: x.angle + angle_delta(x.angle, y.angle) * w,
                    scale: lerp2(x.scale, y.scale, w),
                })
                .collect(),
        }
    }

    /// Forward kinematics: every bone's transform under `root`.
    #[must_use]
    pub fn world(&self, pose: &Pose, root: &Xform2) -> Vec<Xform2> {
        let mut out: Vec<Xform2> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let local = pose.local.get(i).copied().unwrap_or(b.rest).xform();
            let parent = b.parent.and_then(|p| out.get(p).copied()).unwrap_or(*root);
            out.push(local.under(&parent));
        }
        out
    }

    /// A bone's tip in the space of `world`.
    #[must_use]
    pub fn tip(&self, world: &[Xform2], bone: usize) -> Option<DVec2> {
        let x = world.get(bone)?;
        Some(x.apply(DVec2::new(self.bones.get(bone)?.length, 0.0)))
    }

    /// Two-bone IK: turn `upper` and its child `lower` so `lower`'s tip reaches `target`
    /// (in the space `root` maps into), bending counter-clockwise when `ccw`. Out of reach:
    /// the chain points straight at the target. Uniform scale assumed on the chain.
    pub fn two_bone_ik(
        &self,
        pose: &mut Pose,
        root: &Xform2,
        upper: usize,
        lower: usize,
        target: DVec2,
        ccw: bool,
    ) -> Result<(), Error2d> {
        if self.bones.get(lower).and_then(|b| b.parent) != Some(upper) {
            return Err(Error2d::invalid("two-bone IK needs a bone and its child"));
        }
        let world = self.world(pose, root);
        let up = world[upper];
        let parent = self.bones[upper].parent.map_or(*root, |p| world[p]);
        let l1 = self.bones[upper].length * up.scale.x.abs();
        let l2 = self.bones[lower].length * world[lower].scale.x.abs();
        let d = target - up.translation;
        let dist = d.length().clamp((l1 - l2).abs() + 1e-9, l1 + l2 - 1e-9);
        // Law of cosines for the angle at the upper joint, as an atan2 (no acos in det).
        let cos_a = ((l1 * l1 + dist * dist - l2 * l2) / (2.0 * l1 * dist)).clamp(-1.0, 1.0);
        let a = det::atan2(det::sqrt(1.0 - cos_a * cos_a), cos_a);
        let cos_b = ((l1 * l1 + l2 * l2 - dist * dist) / (2.0 * l1 * l2)).clamp(-1.0, 1.0);
        let b = det::atan2(det::sqrt(1.0 - cos_b * cos_b), cos_b);
        let aim = det::atan2(d.y, d.x);
        let sign = if ccw { 1.0 } else { -1.0 };
        let upper_world = aim - sign * a;
        let parent_angle = parent.rot.angle();
        pose.local[upper].angle = upper_world - parent_angle;
        // The lower bone turns by (pi - b) the other way relative to the upper.
        pose.local[lower].angle = sign * (core::f64::consts::PI - b);
        Ok(())
    }

    /// Sprites for `parts` posed by `world` (from [`Self::world`] with a root placed at
    /// `origin`'s local offset), in `origin`'s frame, on `layer` with order `base_order + z`.
    #[must_use]
    pub fn sprites(
        parts: &[Part],
        world: &[Xform2],
        origin: FramePos2,
        layer: i32,
        base_order: i32,
    ) -> Vec<Sprite> {
        let mut out = Vec::with_capacity(parts.len());
        for p in parts {
            let Some(x) = world.get(p.bone) else { continue };
            let at = x.apply(p.offset);
            let angle = x.rot.angle() + p.angle;
            out.push(Sprite {
                pos: FramePos2::new(origin.frame, at),
                size: p.size.mul_elem(x.scale),
                pivot: p.pivot,
                angle,
                uv: p.uv,
                texture: p.texture,
                tint: [1.0; 4],
                layer,
                order: base_order + p.z,
                flip_x: false,
                flip_y: false,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm() -> Skeleton2d {
        Skeleton2d {
            bones: vec![
                Bone2d {
                    name: "upper".into(),
                    parent: None,
                    rest: BoneLocal::default(),
                    length: 2.0,
                },
                Bone2d {
                    name: "lower".into(),
                    parent: Some(0),
                    rest: BoneLocal {
                        offset: DVec2::new(2.0, 0.0),
                        ..BoneLocal::default()
                    },
                    length: 1.5,
                },
            ],
        }
    }

    #[test]
    fn forward_kinematics_chains_rotations() {
        let s = arm();
        let mut p = s.rest_pose();
        p.local[0].angle = core::f64::consts::FRAC_PI_2;
        let w = s.world(&p, &Xform2::IDENTITY);
        let tip = s.tip(&w, 1).expect("tip");
        assert!((tip - DVec2::new(0.0, 3.5)).length() < 1e-12, "{tip:?}");
    }

    #[test]
    fn clips_interpolate_keys_and_loop() {
        let s = arm();
        let clip = Clip {
            name: "wave".into(),
            duration_us: 1_000_000,
            looping: true,
            tracks: vec![Track {
                bone: 1,
                rotation: vec![(0, 0.0), (500_000, 1.0), (1_000_000, 0.0)],
                ..Track::default()
            }],
        };
        assert!((s.sample(&clip, 250_000).local[1].angle - 0.5).abs() < 1e-12);
        assert!(
            (s.sample(&clip, 1_250_000).local[1].angle - 0.5).abs() < 1e-12,
            "loops"
        );
        let half = Skeleton2d::blend(&s.rest_pose(), &s.sample(&clip, 500_000), 0.5);
        assert!((half.local[1].angle - 0.5).abs() < 1e-12);
    }

    #[test]
    fn two_bone_ik_reaches_a_reachable_target_bending_either_way() {
        let s = arm();
        for ccw in [true, false] {
            let mut p = s.rest_pose();
            let target = DVec2::new(1.5, 1.8);
            s.two_bone_ik(&mut p, &Xform2::IDENTITY, 0, 1, target, ccw)
                .expect("ik");
            let w = s.world(&p, &Xform2::IDENTITY);
            let tip = s.tip(&w, 1).expect("tip");
            assert!((tip - target).length() < 1e-9, "ccw {ccw}: {tip:?}");
            let elbow = w[1].translation;
            let side = target.cross(elbow);
            assert_eq!(side < 0.0, ccw, "bends to the chosen side");
        }
        let mut p = s.rest_pose();
        s.two_bone_ik(&mut p, &Xform2::IDENTITY, 0, 1, DVec2::new(10.0, 0.0), true)
            .expect("ik");
        let tip = s.tip(&s.world(&p, &Xform2::IDENTITY), 1).expect("tip");
        assert!(
            (tip - DVec2::new(3.5, 0.0)).length() < 1e-4,
            "out of reach: straight at it"
        );
    }

    #[test]
    fn unordered_bones_sort_parents_first_and_name_loops() {
        let b = |n: &str, p: Option<&str>| {
            (
                n.to_string(),
                p.map(str::to_string),
                BoneLocal::default(),
                1.0,
            )
        };
        let (sk, idx, broken) = Skeleton2d::from_unordered(&[
            b("hand", Some("arm")),
            b("arm", Some("root")),
            b("root", None),
            b("a", Some("b")),
            b("b", Some("a")),
            b("orphan", Some("missing")),
        ]);
        sk.validate().expect("parents first");
        assert_eq!(sk.bones.len(), 3);
        assert_eq!(sk.bones[idx["hand"]].parent, Some(idx["arm"]));
        assert_eq!(broken, vec!["a", "b", "orphan"]);
    }
}
