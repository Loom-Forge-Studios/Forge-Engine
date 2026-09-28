//! The resolver seam: how every consumer turns a [`FramePos`] into a position in another
//! frame, and what a view into those frames needs from the renderer's depth.

use forge_num::DQuat;

use crate::{FrameError, FrameId, FramePos, Tick};

/// Resolves positions between the frames it knows.
///
/// Every consumer of positions — the renderer's last mile, the editor's camera, picking,
/// gizmos, measuring — goes through this trait, so none of them assumes how many frames
/// there are. [`WorldFrame`] is the resolver of a single-frame world; a resolver with more
/// frames implements the same four questions.
///
/// Every method is **pure and deterministic**: the same arguments give the same bits on every
/// platform, at any [`Tick`], forwards or backwards. Resolving a position into the frame it
/// is already in returns it bit-for-bit.
pub trait FrameResolver: Send + Sync {
    /// Does the resolver know `f`?
    fn contains(&self, f: FrameId) -> bool;

    /// `p` expressed in frame `to` at `t`.
    fn resolve(&self, p: FramePos, to: FrameId, t: Tick) -> Result<FramePos, FrameError>;

    /// The rotation taking `f`'s axes to the axes of the outermost frame `f` is placed in,
    /// and that frame. Two frames with the same outermost frame are connected.
    fn rotation_to_root(&self, f: FrameId, t: Tick) -> Result<(DQuat, FrameId), FrameError>;

    /// The frame `f` is placed in, if frames nest (`None` for an outermost frame, and for
    /// every frame of a single-frame world).
    fn enclosing(&self, f: FrameId) -> Option<FrameId> {
        let _ = f;
        None
    }

    /// The depth passes a view into these frames is drawn in (the renderer's depth layout).
    fn view_depth(&self) -> ViewDepth {
        ViewDepth::WORLD
    }
}

/// The resolver of a single-frame world: [`FrameId::WORLD`] and nothing else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorldFrame;

impl FrameResolver for WorldFrame {
    fn contains(&self, f: FrameId) -> bool {
        f == FrameId::WORLD
    }

    fn resolve(&self, p: FramePos, to: FrameId, _t: Tick) -> Result<FramePos, FrameError> {
        if p.frame != FrameId::WORLD {
            return Err(FrameError::UnknownFrame(p.frame));
        }
        if to != FrameId::WORLD {
            return Err(FrameError::UnknownFrame(to));
        }
        Ok(p)
    }

    fn rotation_to_root(&self, f: FrameId, _t: Tick) -> Result<(DQuat, FrameId), FrameError> {
        if f == FrameId::WORLD {
            Ok((DQuat::IDENTITY, FrameId::WORLD))
        } else {
            Err(FrameError::UnknownFrame(f))
        }
    }
}

/// One depth pass: the view distances it draws, metres, and its names in the renderer's
/// frame graph (the pass, and its depth attachment).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthSpan {
    /// Near plane, metres.
    pub near: f64,
    /// Far plane, metres.
    pub far: f64,
    /// The render-graph pass that draws it.
    pub pass: &'static str,
    /// Its depth attachment.
    pub attachment: &'static str,
}

/// How a view into a resolver's frames is split into depth passes: at most three, **back to
/// front**, each drawn with its own depth clear (reversed-Z), and optionally the distance
/// beyond which a mesh instance becomes a sky sprite instead of being culled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewDepth {
    /// The passes, back to front.
    pub passes: &'static [DepthSpan],
    /// Beyond this distance a mesh instance is drawn as a sky sprite; `None`: culled at the
    /// last pass's far plane.
    pub sprites_beyond: Option<f64>,
}

impl ViewDepth {
    /// Nearest distance anything is drawn at, metres.
    pub const NEAREST: f64 = 0.01;

    /// The world frame's depth: **one** reversed-Z pass from 1 cm to 1e4 km. A 32-bit float
    /// depth buffer under a reversed projection keeps ~1e-7 relative precision over the whole
    /// range, so a world tens of kilometres wide needs no second pass.
    pub const WORLD: ViewDepth = ViewDepth {
        passes: &[DepthSpan {
            near: Self::NEAREST,
            far: 1e7,
            pass: "scene",
            attachment: "depth.scene",
        }],
        sprites_beyond: None,
    };

    /// The nearest pass's near plane (the nearest distance anything is drawn at).
    #[must_use]
    pub fn nearest(&self) -> f64 {
        self.passes.last().map_or(Self::NEAREST, |p| p.near)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DVec3;

    #[test]
    fn the_world_frame_resolves_only_itself_and_bit_exactly() {
        let w = WorldFrame;
        let p = FramePos::new(FrameId::WORLD, DVec3::new(6.371e6, -0.1, 1e-300));
        assert_eq!(w.resolve(p, FrameId::WORLD, Tick(7)), Ok(p));
        assert_eq!(
            w.resolve(p, FrameId(3), Tick(0)),
            Err(FrameError::UnknownFrame(FrameId(3)))
        );
        assert_eq!(
            w.resolve(FramePos::origin_of(FrameId(2)), FrameId::WORLD, Tick(0)),
            Err(FrameError::UnknownFrame(FrameId(2)))
        );
        assert_eq!(
            w.rotation_to_root(FrameId::WORLD, Tick(0)),
            Ok((DQuat::IDENTITY, FrameId::WORLD))
        );
        assert!(w.rotation_to_root(FrameId(1), Tick(0)).is_err());
        assert!(w.contains(FrameId::WORLD) && !w.contains(FrameId(1)));
        assert_eq!(w.enclosing(FrameId::WORLD), None);
        assert_eq!(w.view_depth(), ViewDepth::WORLD);
    }

    #[test]
    fn the_world_depth_is_one_pass_from_a_centimetre() {
        let d = ViewDepth::WORLD;
        assert_eq!(d.passes.len(), 1);
        assert_eq!(d.nearest(), ViewDepth::NEAREST);
        assert!(
            d.passes[0].far > 1e5,
            "a world tens of km wide fits in the pass"
        );
        assert_eq!(d.sprites_beyond, None);
    }
}
