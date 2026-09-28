//! A [`FrameResolver`] double for testing the seam's consumers against frames that are not
//! the world frame: frames placed at a **fixed** offset and orientation in the world frame,
//! one level deep. It is test support, not an engine feature — nothing moves, nothing nests.

use forge_num::{DQuat, DVec3};

use crate::{FrameError, FrameId, FramePos, FrameResolver, Tick, ViewDepth};

/// The world frame plus fixed frames placed in it (`FrameId(1)`, `FrameId(2)`, … in the
/// order added).
#[derive(Clone, Debug)]
pub struct FixedFrames {
    /// `(offset in the world frame, the frame's axes -> the world's axes)`.
    frames: Vec<(DVec3, DQuat)>,
    depth: ViewDepth,
}

impl Default for FixedFrames {
    fn default() -> Self {
        Self::new()
    }
}

impl FixedFrames {
    /// Only the world frame.
    #[must_use]
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            depth: ViewDepth::WORLD,
        }
    }

    /// Place a frame at `offset` in the world frame, its axes turned by `rotation` (a unit
    /// quaternion: the frame's axes to the world's).
    pub fn add(&mut self, offset: DVec3, rotation: DQuat) -> FrameId {
        self.frames.push((offset, rotation));
        FrameId(self.frames.len() as u32)
    }

    /// Report `depth` as the view depth.
    #[must_use]
    pub fn with_view_depth(mut self, depth: ViewDepth) -> Self {
        self.depth = depth;
        self
    }

    fn placed(&self, f: FrameId) -> Option<(DVec3, DQuat)> {
        (f.0 as usize)
            .checked_sub(1)
            .and_then(|i| self.frames.get(i).copied())
    }

    fn to_world(&self, p: FramePos) -> Result<DVec3, FrameError> {
        if p.frame == FrameId::WORLD {
            return Ok(p.local);
        }
        let (t, r) = self
            .placed(p.frame)
            .ok_or(FrameError::UnknownFrame(p.frame))?;
        Ok(t + r.rotate(p.local))
    }
}

impl FrameResolver for FixedFrames {
    fn contains(&self, f: FrameId) -> bool {
        f == FrameId::WORLD || self.placed(f).is_some()
    }

    fn resolve(&self, p: FramePos, to: FrameId, _t: Tick) -> Result<FramePos, FrameError> {
        if !self.contains(to) {
            return Err(FrameError::UnknownFrame(to));
        }
        if p.frame == to {
            return if self.contains(p.frame) {
                Ok(p)
            } else {
                Err(FrameError::UnknownFrame(p.frame))
            };
        }
        let w = self.to_world(p)?;
        if to == FrameId::WORLD {
            return Ok(FramePos::new(to, w));
        }
        let (t, r) = self.placed(to).ok_or(FrameError::UnknownFrame(to))?;
        Ok(FramePos::new(to, r.inverse_rotate(w - t)))
    }

    fn rotation_to_root(&self, f: FrameId, _t: Tick) -> Result<(DQuat, FrameId), FrameError> {
        if f == FrameId::WORLD {
            return Ok((DQuat::IDENTITY, FrameId::WORLD));
        }
        let (_, r) = self.placed(f).ok_or(FrameError::UnknownFrame(f))?;
        Ok((r, FrameId::WORLD))
    }

    fn enclosing(&self, f: FrameId) -> Option<FrameId> {
        self.placed(f).map(|_| FrameId::WORLD)
    }

    fn view_depth(&self) -> ViewDepth {
        self.depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_frame_resolves_both_ways() {
        let mut f = FixedFrames::new();
        let q = DQuat::from_axis_angle(DVec3::Z, core::f64::consts::FRAC_PI_2);
        let a = f.add(DVec3::new(10.0, 0.0, 0.0), q);
        let b = f.add(DVec3::new(0.0, -5.0, 0.0), DQuat::IDENTITY);
        let p = FramePos::new(a, DVec3::X);
        let w = f.resolve(p, FrameId::WORLD, Tick(0)).expect("to world");
        assert!((w.local - DVec3::new(10.0, 1.0, 0.0)).length() < 1e-12);
        let back = f.resolve(w, a, Tick(0)).expect("back");
        assert!((back.local - DVec3::X).length() < 1e-12);
        let in_b = f.resolve(p, b, Tick(0)).expect("across");
        assert!((in_b.local - DVec3::new(10.0, 6.0, 0.0)).length() < 1e-12);
        assert_eq!(f.rotation_to_root(a, Tick(0)), Ok((q, FrameId::WORLD)));
        assert_eq!(f.enclosing(a), Some(FrameId::WORLD));
        assert_eq!(
            f.resolve(p, FrameId(9), Tick(0)),
            Err(FrameError::UnknownFrame(FrameId(9)))
        );
        assert_eq!(f.resolve(p, a, Tick(0)), Ok(p), "same frame is bit-exact");
    }
}
