//! `forge-frames` — coordinates, frames and time (Ch.2). **This crate is the engine**:
//! everything that has a position is downstream of it.
//!
//! * There is **no global position type** (invariant I1). A position is a [`FramePos`] — a
//!   [`FrameId`] plus a frame-local `DVec3` — and a velocity is a [`FrameVel`]. The I1 lint
//!   (`tests/liveness/test_frame_liveness.rs`) rejects a bare `DVec3` in any position-shaped
//!   public signature anywhere in the workspace.
//! * **One world frame** ([`FrameId::WORLD`], [`WorldFrame`]): every position is an `f64`
//!   offset from the world origin, precise to well under a millimetre across a world tens of
//!   kilometres wide, and the renderer draws camera-relative (subtract in `f64`, then narrow).
//!   That is the large-world model of Unreal and Godot, and enough for any single-map game.
//! * [`FrameResolver`] is the **seam** every consumer resolves positions through: the
//!   renderer, the editor viewport, picking and measuring never assume there is only one
//!   frame, so a resolver with more frames plugs in without touching them. [`WorldFrame`] is
//!   the resolver of the world frame.
//! * [`Tick`] is canonical time: an `i64` count of microseconds from the epoch (ADR 0004).

#![forbid(unsafe_code)]

mod error;
mod resolve;
#[doc(hidden)]
pub mod testing;
mod time;

pub use error::{ExtensionError, FrameError};
pub use forge_num::{DQuat, DVec2, DVec3};
pub use resolve::{DepthSpan, FrameResolver, ViewDepth, WorldFrame};
pub use time::{TICKS_PER_SECOND, Tick};

/// Identifies a frame. The world frame is [`FrameId::WORLD`]; a resolver assigns any others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, bevy_reflect::Reflect)]
#[reflect(opaque, Clone, Debug, PartialEq, Hash)]
pub struct FrameId(pub u32);

impl FrameId {
    /// The world frame: the origin every position of a single-frame world is measured from.
    pub const WORLD: FrameId = FrameId(0);
}

/// **The only legal way to express a position anywhere in the engine** (I1): the frame it
/// is expressed in, and the offset from that frame's origin along that frame's axes.
/// There is no `WorldPos`, no `GlobalPos`, and no bare `DVec3` in any public signature.
// Reflected opaque: `DVec3` lives in forge-num, which depends on nothing (Ch.1.1), and remote
// reflection would need `unsafe` (forbidden here). The inspector edits it with a vector widget
// and `forge-reflect` supplies its JSON Schema (ADR 0008).
#[derive(Clone, Copy, Debug, PartialEq, bevy_reflect::Reflect)]
#[reflect(opaque, Clone, Debug, PartialEq)]
pub struct FramePos {
    pub frame: FrameId,
    pub local: DVec3,
}

impl FramePos {
    #[inline]
    pub const fn new(frame: FrameId, local: DVec3) -> Self {
        Self { frame, local }
    }

    /// The frame's own origin.
    #[inline]
    pub const fn origin_of(frame: FrameId) -> Self {
        Self::new(frame, DVec3::ZERO)
    }
}

/// A 2D position (Ch.35 §35.3): the same discipline as [`FramePos`] — the frame it is
/// expressed in and the offset from that frame's origin — for the 2D pipeline, which always
/// works in one frame. The frame is still carried, so 2D content promoted into a 3D project
/// (Ch.31.4) keeps meaning what it meant (`to_3d` puts it on `z = 0`).
#[derive(Clone, Copy, Debug, PartialEq, bevy_reflect::Reflect)]
#[reflect(opaque, Clone, Debug, PartialEq)]
pub struct FramePos2 {
    pub frame: FrameId,
    pub local: DVec2,
}

impl FramePos2 {
    #[inline]
    pub const fn new(frame: FrameId, local: DVec2) -> Self {
        Self { frame, local }
    }

    /// The frame's own origin.
    #[inline]
    pub const fn origin_of(frame: FrameId) -> Self {
        Self::new(frame, DVec2::ZERO)
    }

    /// The same point as a 3D [`FramePos`] on the frame's `z = 0` plane.
    #[inline]
    pub const fn to_3d(self) -> FramePos {
        FramePos::new(self.frame, DVec3::new(self.local.x, self.local.y, 0.0))
    }
}

/// A velocity: the rate of change of a [`FramePos`] as observed *in* `frame`.
#[derive(Clone, Copy, Debug, PartialEq, bevy_reflect::Reflect)]
#[reflect(opaque, Clone, Debug, PartialEq)]
pub struct FrameVel {
    pub frame: FrameId,
    pub local: DVec3,
}

impl FrameVel {
    #[inline]
    pub const fn new(frame: FrameId, local: DVec3) -> Self {
        Self { frame, local }
    }

    /// Zero velocity in `frame` (at rest relative to it).
    #[inline]
    pub const fn at_rest(frame: FrameId) -> Self {
        Self::new(frame, DVec3::ZERO)
    }
}
