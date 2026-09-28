//! The pixel-perfect camera with integer scaling (Ch.35 §35.2).
//!
//! A pixel-art game draws its world into a small **reference** target (320x180, say), one
//! texel per art pixel, then scales that image up by the **largest whole number** that fits
//! the window and centres it with bars — never a fractional scale, so no art pixel is ever
//! drawn one screen pixel wider than its neighbour. The camera's position **snaps** to the
//! reference pixel grid, so a scrolling world moves by whole art pixels and never shimmers.
//! Without a reference size the camera draws at the output resolution (a smooth-art game).
//!
//! Everything here is `f64` and pure; the renderer's last mile narrows the result.

use forge_frames::{FrameId, FramePos2};
use forge_num::DVec2;

use crate::Error2d;

/// See the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2d {
    /// The world point at the centre of the view.
    pub pos: FramePos2,
    /// Reference pixels per world unit (the art's pixels per unit: 16 for 16-px tiles one
    /// unit wide).
    pub pixels_per_unit: f64,
    /// Multiplies `pixels_per_unit` (an integer keeps pixel art crisp).
    pub zoom: f64,
    /// The pixel-perfect reference size; `None`: draw at the output size.
    pub reference: Option<(u32, u32)>,
    /// Snap `pos` to the reference pixel grid.
    pub snap: bool,
}

/// Where the scaled reference image sits in the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    /// The whole-number scale.
    pub scale: u32,
    /// Top-left corner and size in output pixels (may extend past the output when the
    /// window is smaller than the reference: then the image is cropped, centred).
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
}

impl Camera2d {
    /// A pixel-perfect camera at `pos`: `reference` pixels, `pixels_per_unit`, snapped.
    #[must_use]
    pub fn pixel_perfect(pos: FramePos2, reference: (u32, u32), pixels_per_unit: f64) -> Self {
        Self {
            pos,
            pixels_per_unit,
            zoom: 1.0,
            reference: Some(reference),
            snap: true,
        }
    }

    /// A camera drawing at the output resolution.
    #[must_use]
    pub fn smooth(pos: FramePos2, pixels_per_unit: f64) -> Self {
        Self {
            pos,
            pixels_per_unit,
            zoom: 1.0,
            reference: None,
            snap: false,
        }
    }

    /// The frame it looks into.
    #[must_use]
    pub fn frame(&self) -> FrameId {
        self.pos.frame
    }

    /// Checks the camera is usable.
    pub fn validate(&self) -> Result<(), Error2d> {
        if !(self.pixels_per_unit > 0.0 && self.pixels_per_unit.is_finite())
            || !(self.zoom > 0.0 && self.zoom.is_finite())
            || !self.pos.local.is_finite()
        {
            return Err(Error2d::invalid(
                "camera: pixels_per_unit and zoom must be positive and finite",
            ));
        }
        if let Some((w, h)) = self.reference
            && (w == 0 || h == 0)
        {
            return Err(Error2d::invalid("camera: a zero reference size"));
        }
        Ok(())
    }

    /// Pixels per world unit after zoom.
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.pixels_per_unit * self.zoom
    }

    /// The size the world is drawn at for an `out_w x out_h` output.
    #[must_use]
    pub fn render_size(&self, out_w: u32, out_h: u32) -> (u32, u32) {
        self.reference.unwrap_or((out_w.max(1), out_h.max(1)))
    }

    /// The largest whole-number scale of the reference that fits the output (at least 1),
    /// centred.
    #[must_use]
    pub fn fit(&self, out_w: u32, out_h: u32) -> Viewport {
        let (rw, rh) = self.render_size(out_w, out_h);
        let scale = (out_w / rw).min(out_h / rh).max(1);
        let (w, h) = (rw * scale, rh * scale);
        Viewport {
            scale,
            x: (i64::from(out_w) - i64::from(w)) / 2,
            y: (i64::from(out_h) - i64::from(h)) / 2,
            w,
            h,
        }
    }

    /// The view centre actually used: `pos`, snapped to the pixel grid when `snap`.
    #[must_use]
    pub fn center(&self) -> FramePos2 {
        if !self.snap {
            return self.pos;
        }
        let s = self.scale();
        FramePos2::new(
            self.pos.frame,
            DVec2::new(
                (self.pos.local.x * s).round() / s,
                (self.pos.local.y * s).round() / s,
            ),
        )
    }

    /// Half the view's size in world units for a `rw x rh` render size.
    #[must_use]
    pub fn half_extent(&self, rw: u32, rh: u32) -> DVec2 {
        let s = self.scale();
        DVec2::new(f64::from(rw) * 0.5 / s, f64::from(rh) * 0.5 / s)
    }

    /// A world point in render-target pixels (x right, y down; `(0, 0)` the top-left
    /// corner of the reference target).
    pub fn to_pixels(&self, p: FramePos2, rw: u32, rh: u32) -> Result<DVec2, Error2d> {
        if p.frame != self.pos.frame {
            return Err(Error2d::Frame {
                why: format!(
                    "point in frame {:?}; the camera is in {:?}",
                    p.frame, self.pos.frame
                ),
            });
        }
        let s = self.scale();
        let d = (p.local - self.center().local) * s;
        Ok(DVec2::new(
            f64::from(rw) * 0.5 + d.x,
            f64::from(rh) * 0.5 - d.y,
        ))
    }

    /// The world point under output pixel `(sx, sy)` (picking), or `None` in the bars.
    #[must_use]
    pub fn screen_to_world(&self, sx: f64, sy: f64, out_w: u32, out_h: u32) -> Option<FramePos2> {
        let vp = self.fit(out_w, out_h);
        let (rw, rh) = self.render_size(out_w, out_h);
        let px = (sx - vp.x as f64) / f64::from(vp.scale);
        let py = (sy - vp.y as f64) / f64::from(vp.scale);
        if px < 0.0 || py < 0.0 || px > f64::from(rw) || py > f64::from(rh) {
            return None;
        }
        let s = self.scale();
        let c = self.center().local;
        Some(FramePos2::new(
            self.pos.frame,
            DVec2::new(
                c.x + (px - f64::from(rw) * 0.5) / s,
                c.y - (py - f64::from(rh) * 0.5) / s,
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera2d {
        Camera2d::pixel_perfect(
            FramePos2::new(FrameId(0), DVec2::new(10.03, 4.0)),
            (320, 180),
            16.0,
        )
    }

    #[test]
    fn integer_scaling_picks_the_largest_whole_scale_and_centres() {
        let c = cam();
        assert_eq!(
            c.fit(1920, 1080),
            Viewport {
                scale: 6,
                x: 0,
                y: 0,
                w: 1920,
                h: 1080
            }
        );
        let v = c.fit(1280, 720);
        assert_eq!((v.scale, v.x, v.y), (4, 0, 0));
        let v = c.fit(1000, 700);
        assert_eq!(
            (v.scale, v.w, v.h, v.x, v.y),
            (3, 960, 540, 20, 80),
            "bars, never a fractional scale"
        );
        assert_eq!(
            c.fit(200, 100).scale,
            1,
            "smaller than the reference: 1x, cropped"
        );
    }

    #[test]
    fn the_camera_snaps_to_whole_art_pixels() {
        let c = cam();
        let s = c.center().local;
        assert_eq!(s.x * 16.0, (s.x * 16.0).round());
        assert!((s.x - 10.0).abs() < 1.0 / 16.0);
        let p = c
            .to_pixels(FramePos2::new(FrameId(0), DVec2::new(11.0, 4.0)), 320, 180)
            .expect("px");
        assert_eq!(p.x, p.x.round(), "a whole-unit point lands on a pixel edge");
        let unsnapped = Camera2d { snap: false, ..c };
        let q = unsnapped
            .to_pixels(FramePos2::new(FrameId(0), DVec2::new(11.0, 4.0)), 320, 180)
            .expect("px");
        assert_ne!(
            q.x,
            q.x.round(),
            "the control: without snapping it straddles pixels"
        );
    }

    #[test]
    fn picking_inverts_the_projection() {
        let c = cam();
        let w = FramePos2::new(FrameId(0), DVec2::new(12.5, 3.25));
        let p = c.to_pixels(w, 320, 180).expect("px");
        let vp = c.fit(1280, 720);
        let back = c
            .screen_to_world(vp.x as f64 + p.x * 4.0, vp.y as f64 + p.y * 4.0, 1280, 720)
            .expect("inside");
        assert!((back.local - w.local).length() < 1e-9);
        assert!(
            c.to_pixels(FramePos2::new(FrameId(2), DVec2::ZERO), 320, 180)
                .is_err()
        );
    }
}
