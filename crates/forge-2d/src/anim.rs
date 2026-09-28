//! Sprite sheets and frame animation (Ch.35 §35.2).
//!
//! A sheet is cut into frames by a grid ([`SheetSlice`]: cell size, offset, spacing —
//! cells crossing the image edge are not frames). An animation ([`FrameAnim`]) is a list of
//! frames, each held for its own duration (Aseprite gives per-frame durations; a sheet
//! animation from the editor has one rate), played forward, backward, ping-pong, or once.
//! Time is whole **microseconds** (`forge_frames::Tick` units): a frame index is an integer
//! function of an integer time, with no floating-point drift over a long session.

use serde::{Deserialize, Serialize};

use crate::Error2d;

/// A grid cut (pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetSlice {
    pub image_w: u32,
    pub image_h: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub spacing_x: u32,
    pub spacing_y: u32,
}

/// One frame of a sliced sheet (pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRect {
    pub index: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The frames a slicing yields, row-major.
#[must_use]
pub fn slice(s: &SheetSlice) -> Vec<FrameRect> {
    let mut out = Vec::new();
    if s.cell_w == 0 || s.cell_h == 0 {
        return out;
    }
    let mut index = 0u32;
    let mut y = s.offset_y;
    while y + s.cell_h <= s.image_h {
        let mut x = s.offset_x;
        while x + s.cell_w <= s.image_w {
            out.push(FrameRect {
                index,
                x,
                y,
                w: s.cell_w,
                h: s.cell_h,
            });
            index += 1;
            x += s.cell_w + s.spacing_x;
        }
        y += s.cell_h + s.spacing_y;
    }
    out
}

/// How an animation plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Playback {
    #[default]
    Loop,
    /// Plays once and holds the last frame.
    Once,
    /// Forward then backward, the end frames not repeated.
    PingPong,
    /// Backward, looping.
    Reverse,
}

/// A frame animation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameAnim {
    pub name: String,
    /// Frame indices into the sheet.
    pub frames: Vec<u32>,
    /// How long each frame shows (microseconds), one per frame.
    pub durations_us: Vec<u64>,
    pub playback: Playback,
}

impl FrameAnim {
    /// `frames` at a constant `fps`.
    pub fn at_fps(
        name: &str,
        frames: Vec<u32>,
        fps: f64,
        playback: Playback,
    ) -> Result<Self, Error2d> {
        if !(fps > 0.0 && fps.is_finite()) {
            return Err(Error2d::invalid("an animation needs a positive frame rate"));
        }
        let d = (1_000_000.0 / fps).round().max(1.0) as u64;
        let n = frames.len();
        Ok(Self {
            name: name.to_string(),
            frames,
            durations_us: vec![d; n],
            playback,
        })
    }

    /// The sequence of positions one cycle plays (ping-pong adds the way back).
    fn cycle(&self) -> Vec<usize> {
        let n = self.frames.len();
        match self.playback {
            Playback::Loop | Playback::Once => (0..n).collect(),
            Playback::Reverse => (0..n).rev().collect(),
            Playback::PingPong => {
                let mut v: Vec<usize> = (0..n).collect();
                if n > 2 {
                    v.extend((1..n - 1).rev());
                }
                v
            }
        }
    }

    /// Length of one cycle (microseconds).
    #[must_use]
    pub fn cycle_us(&self) -> u64 {
        self.cycle()
            .iter()
            .map(|i| self.durations_us.get(*i).copied().unwrap_or(0))
            .sum()
    }

    /// The sheet frame showing at `t_us` after the start (`None` for an empty animation).
    #[must_use]
    pub fn frame_at(&self, t_us: u64) -> Option<u32> {
        let seq = self.cycle();
        let total = self.cycle_us();
        if seq.is_empty() || total == 0 {
            return self.frames.first().copied();
        }
        let t = if self.playback == Playback::Once {
            if t_us >= total {
                return self.frames.last().copied();
            }
            t_us
        } else {
            t_us % total
        };
        let mut acc = 0;
        for i in &seq {
            acc += self.durations_us.get(*i).copied().unwrap_or(0);
            if t < acc {
                return self.frames.get(*i).copied();
            }
        }
        self.frames.get(*seq.last()?).copied()
    }

    /// Has a `Once` animation finished at `t_us`?
    #[must_use]
    pub fn finished(&self, t_us: u64) -> bool {
        self.playback == Playback::Once && t_us >= self.cycle_us()
    }
}

/// A playing animation: which one and how far in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimPlayer {
    pub current: String,
    pub t_us: u64,
}

impl AnimPlayer {
    /// Switch to `name` (restarting only if it is a different animation).
    pub fn play(&mut self, name: &str) {
        if self.current != name {
            self.current = name.to_string();
            self.t_us = 0;
        }
    }

    /// Advance by `dt_us`.
    pub fn advance(&mut self, dt_us: u64) {
        self.t_us = self.t_us.saturating_add(dt_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicing_respects_offset_spacing_and_edges() {
        let f = slice(&SheetSlice {
            image_w: 100,
            image_h: 50,
            cell_w: 30,
            cell_h: 20,
            offset_x: 2,
            offset_y: 3,
            spacing_x: 2,
            spacing_y: 4,
        });
        assert_eq!(f.len(), 6);
        assert_eq!((f[4].x, f[4].y, f[4].index), (34, 27, 4));
    }

    #[test]
    fn playback_modes() {
        let mut a =
            FrameAnim::at_fps("run", vec![10, 11, 12, 13], 10.0, Playback::Loop).expect("anim");
        let at = |a: &FrameAnim, ms: u64| a.frame_at(ms * 1000);
        assert_eq!(
            [at(&a, 0), at(&a, 99), at(&a, 100), at(&a, 399), at(&a, 400)],
            [Some(10), Some(10), Some(11), Some(13), Some(10)]
        );
        a.playback = Playback::PingPong;
        let seq: Vec<Option<u32>> = (0..7).map(|i| at(&a, i * 100)).collect();
        assert_eq!(seq, [10, 11, 12, 13, 12, 11, 10].map(Some));
        a.playback = Playback::Once;
        assert_eq!(at(&a, 10_000), Some(13));
        assert!(a.finished(400_000) && !a.finished(399_999));
        a.playback = Playback::Reverse;
        assert_eq!(at(&a, 0), Some(13));
        a.durations_us = vec![50_000, 300_000, 50_000, 50_000];
        a.playback = Playback::Loop;
        assert_eq!(
            [at(&a, 60), at(&a, 340), at(&a, 360)],
            [Some(11), Some(11), Some(12)]
        );
    }

    #[test]
    fn a_long_session_never_drifts() {
        let a = FrameAnim::at_fps("idle", vec![0, 1, 2], 12.0, Playback::Loop).expect("anim");
        // Ten hours in, still a pure function of whole microseconds.
        let t = 36_000_000_000u64;
        assert_eq!(a.frame_at(t), a.frame_at(t % a.cycle_us()));
        let mut p = AnimPlayer::default();
        p.play("idle");
        p.advance(5);
        p.play("idle");
        assert_eq!(p.t_us, 5, "the same animation keeps playing");
        p.play("run");
        assert_eq!(p.t_us, 0);
    }
}
