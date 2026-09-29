//! The viewport's engine side (Ch.21 §21.21 "Viewport", DoD M2-32): everything a viewport
//! does that is not drawing widgets, so it is tested headless and shared by the viewport
//! panel (`plugins/forge-panels-scene`) and the editor binary's renderer host.
//!
//! * [`camera`] — the `f64`, camera-relative editor camera: orbit / fly / pan, log-scaled
//!   speed, usable from a centimetre to 1e13 m without jitter.
//! * [`scene`] — drawables read incrementally from the mirror, picking, and the line
//!   overlays (grid, frame axes, bounding boxes) that are also the placeholder renderer.
//! * [`gizmo`] — translate / rotate / scale handles in local, world or frame space, with
//!   snapping, producing `Transform` commands.
//! * [`tool`] — the `ViewportTool` extension point and the built-in select, transform and
//!   measure tools.
//! * [`controller`] — one viewport's input → navigation or the active tool.
//! * [`physics`] — the physics debug drawing during Play: the play core's colliders and
//!   joints as overlay lines.
//! * [`surface`] — how a GPU scene renderer (the binary's `forge-render` host) is attached
//!   to viewports without the editor library touching a GPU.

pub mod camera;
pub mod controller;
pub mod gizmo;
pub mod layer;
pub mod physics;
pub mod scene;
pub mod surface;
pub mod tool;
pub mod world;

/// A distance for people: `12.3 cm`, `4.56 m`, `7.89 km`, `1.23 Mm`.
pub fn format_metres(m: f64) -> String {
    Metres(m).to_string()
}

/// [`format_metres`] as a [`Display`](std::fmt::Display): written straight into a reused
/// buffer (a viewport overlay that paints without allocating).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metres(pub f64);

impl std::fmt::Display for Metres {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = self.0;
        let a = m.abs();
        if a < 1.0 {
            write!(f, "{:.1} cm", m * 100.0)
        } else if a < 1.0e3 {
            write!(f, "{m:.2} m")
        } else if a < 1.0e6 {
            write!(f, "{:.2} km", m / 1.0e3)
        } else {
            write!(f, "{:.2} Mm", m / 1.0e6)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn metres_read_at_every_scale() {
        use super::format_metres;
        assert_eq!(format_metres(0.123), "12.3 cm");
        assert_eq!(format_metres(4.561), "4.56 m");
        assert_eq!(format_metres(7890.0), "7.89 km");
        assert_eq!(format_metres(2.5e6), "2.50 Mm");
        assert_eq!(format_metres(1.0e7), "10.00 Mm");
    }
}
