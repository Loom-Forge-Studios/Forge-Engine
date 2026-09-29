//! The physics debug drawing during Play (WP-60, Ch.17): the play core's colliders and joints
//! ([`forge_phys::debug`]) as overlay lines, camera-relative in `f64` like every overlay, one
//! [`LineStyle::Physics`] per kind (awake, sleeping, kinematic, static, trigger, joint).
//!
//! The play core hands the lines in each physics world's region frame
//! ([`crate::play::PlayBackend::physics_debug`]); each frame is placed relative to the camera
//! once and every line of it is drawn from there. Like the bounding boxes, the overlay is
//! bounded: at most [`MAX_LINES`] lines a cell, in body order.

use forge_frames::{DQuat, DVec3, FrameId, FramePos, FrameResolver, Tick};

pub use forge_phys::{DebugKind, DebugLine};

use crate::viewport::scene::{LineSink, LineStyle};

/// The most physics lines one cell draws (2,000 boxes' worth, twice over).
pub const MAX_LINES: usize = 50_000;

/// Draw `lines` (each in its frame) into `sink`, at most [`MAX_LINES`]. Returns how many
/// lines were drawn (a line in a frame the camera cannot place is skipped; one behind the
/// camera is clipped by the sink).
pub fn draw(
    sink: &mut LineSink<'_>,
    lines: &[(FrameId, DebugLine)],
    tree: &dyn FrameResolver,
    t: Tick,
) -> usize {
    let mut drawn = 0;
    let mut placed: Option<(FrameId, Option<(DVec3, DQuat)>)> = None;
    for (f, l) in lines {
        if drawn == MAX_LINES {
            break;
        }
        if placed.is_none_or(|(pf, _)| pf != *f) {
            let origin = FramePos::new(*f, DVec3::ZERO);
            let at = sink.cam.offset_of(tree, t, origin);
            placed = Some((*f, at.zip(sink.cam.rotation_from(tree, t, *f))));
        }
        let Some((_, Some((off, rot)))) = placed else {
            continue;
        };
        sink.seg(
            off + rot.rotate(l.a),
            off + rot.rotate(l.b),
            LineStyle::Physics(l.kind),
        );
        drawn += 1;
    }
    drawn
}

#[cfg(test)]
mod tests {
    use forge_frames::testing::FixedFrames;

    use super::*;
    use crate::viewport::camera::EditorCamera;
    use crate::viewport::scene::Segment;

    /// A line in a turned, offset frame lands on the pixels its two points land on when each
    /// is placed through the camera on its own; a line in a frame the tree does not know is
    /// skipped.
    #[test]
    fn a_line_lands_where_its_frame_puts_its_points() {
        let mut frames = FixedFrames::new();
        let f = frames.add(
            DVec3::new(1.0, 0.0, -2.0),
            DQuat::from_axis_angle(DVec3::Y, 0.7),
        );
        let cam = EditorCamera::new(FrameId::WORLD);
        let (a, b) = (DVec3::new(0.2, 0.5, 0.0), DVec3::new(-0.3, 0.1, 0.4));
        let kind = DebugKind::Sleeping;
        let lines = [
            (f, DebugLine { a, b, kind }),
            (FrameId(99), DebugLine { a, b, kind }),
        ];
        let sink = |out| LineSink {
            cam: &cam,
            w: 800.0,
            h: 600.0,
            out,
        };
        let mut got: Vec<Segment> = Vec::new();
        assert_eq!(draw(&mut sink(&mut got), &lines, &frames, Tick(0)), 1);
        let off = |p| {
            cam.offset_of(&frames, Tick(0), FramePos::new(f, p))
                .expect("placed")
        };
        let mut want: Vec<Segment> = Vec::new();
        sink(&mut want).seg(off(a), off(b), LineStyle::Physics(kind));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].style, want[0].style);
        let d = |p: (f32, f32), q: (f32, f32)| (p.0 - q.0).abs().max((p.1 - q.1).abs());
        assert!(
            d(got[0].a, want[0].a) < 1e-3 && d(got[0].b, want[0].b) < 1e-3,
            "{got:?} vs {want:?}"
        );
    }

    #[test]
    fn a_cell_draws_at_most_max_lines() {
        let frames = FixedFrames::new();
        let cam = EditorCamera::new(FrameId::WORLD);
        let line = DebugLine {
            a: DVec3::ZERO,
            b: DVec3::X,
            kind: DebugKind::Static,
        };
        let lines = vec![(FrameId::WORLD, line); MAX_LINES + 10];
        let mut out = Vec::new();
        let mut sink = LineSink {
            cam: &cam,
            w: 800.0,
            h: 600.0,
            out: &mut out,
        };
        assert_eq!(draw(&mut sink, &lines, &frames, Tick(0)), MAX_LINES);
        assert_eq!(out.len(), MAX_LINES);
    }
}
