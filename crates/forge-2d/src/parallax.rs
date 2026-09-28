//! Parallax layers (Ch.35 §35.2).
//!
//! A layer with factor `f` moves `f` times as fast as the camera: `1` is the world, `0` is
//! fixed to the screen (a sky), `0.3` is a distant mountain range, `1.2` a foreground
//! bush. A repeating layer tiles its content every `repeat` world units so a short strip of
//! background covers an endless level. The renderer applies [`ParallaxLayer::view_offset`]
//! to every sprite on the layer and [`ParallaxLayer::copies`] to repeat it; both are `f64`.

use forge_num::DVec2;

/// See the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParallaxLayer {
    /// Motion relative to the camera, per axis.
    pub factor: DVec2,
    /// Repeat the layer's content every this many world units per axis (`0`: no repeat).
    pub repeat: DVec2,
}

impl Default for ParallaxLayer {
    fn default() -> Self {
        Self {
            factor: DVec2::new(1.0, 1.0),
            repeat: DVec2::ZERO,
        }
    }
}

impl ParallaxLayer {
    /// A layer moving `fx`, `fy` times the camera, not repeating.
    #[must_use]
    pub fn new(fx: f64, fy: f64) -> Self {
        Self {
            factor: DVec2::new(fx, fy),
            repeat: DVec2::ZERO,
        }
    }

    /// The same layer repeating every `rx`, `ry` units.
    #[must_use]
    pub fn repeating(self, rx: f64, ry: f64) -> Self {
        Self {
            repeat: DVec2::new(rx.max(0.0), ry.max(0.0)),
            ..self
        }
    }

    /// What to add to a point of this layer before projecting it with a camera centred at
    /// `camera`: the layer is dragged along by `camera * (1 - factor)`.
    #[must_use]
    pub fn view_offset(&self, camera: DVec2) -> DVec2 {
        DVec2::new(
            camera.x * (1.0 - self.factor.x),
            camera.y * (1.0 - self.factor.y),
        )
    }

    /// For content at `at` (after [`Self::view_offset`]) reaching `reach` units, the
    /// offsets of every copy that can be seen in a view centred at `camera` of half size
    /// `half`: `k * repeat` for each whole `k` that brings a copy into view (just `0` when
    /// the layer does not repeat on an axis).
    #[must_use]
    pub fn copies(&self, at: DVec2, reach: f64, camera: DVec2, half: DVec2) -> Vec<DVec2> {
        let mut out = Vec::new();
        self.for_each_copy(at, reach, camera, half, |d| out.push(d));
        out
    }

    /// [`Self::copies`] without a list: `f` is called with each copy's offset, rows then
    /// columns (the renderer's last mile, which allocates nothing per sprite).
    pub fn for_each_copy(
        &self,
        at: DVec2,
        reach: f64,
        camera: DVec2,
        half: DVec2,
        mut f: impl FnMut(DVec2),
    ) {
        let range = |p: f64, c: f64, h: f64, r: f64| -> (i64, i64) {
            if r <= 0.0 {
                return (0, 0);
            }
            (
                ((c - h - reach - p) / r).ceil() as i64,
                ((c + h + reach - p) / r).floor() as i64,
            )
        };
        let (rx, ry) = (self.repeat.x.max(0.0), self.repeat.y.max(0.0));
        let (x0, x1) = range(at.x, camera.x, half.x, rx);
        let (y0, y1) = range(at.y, camera.y, half.y, ry);
        for ky in y0..=y1 {
            for kx in x0..=x1 {
                f(DVec2::new(kx as f64 * rx, ky as f64 * ry));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors_move_layers_at_their_speed() {
        let sky = ParallaxLayer::new(0.0, 0.0);
        let cam = DVec2::new(100.0, 20.0);
        // A sky point at the origin stays at the view centre: offset == camera.
        assert_eq!(sky.view_offset(cam), cam);
        assert_eq!(ParallaxLayer::default().view_offset(cam), DVec2::ZERO);
        let far = ParallaxLayer::new(0.25, 1.0);
        assert_eq!(far.view_offset(cam), DVec2::new(75.0, 0.0));
    }

    #[test]
    fn repeats_cover_the_view_and_nothing_more() {
        let l = ParallaxLayer::new(0.5, 1.0).repeating(10.0, 0.0);
        let c = l.copies(
            DVec2::ZERO,
            5.0,
            DVec2::new(1000.0, 0.0),
            DVec2::new(20.0, 10.0),
        );
        let xs: Vec<f64> = c.iter().map(|d| d.x).collect();
        assert_eq!(xs, vec![980.0, 990.0, 1000.0, 1010.0, 1020.0]);
        assert!(c.iter().all(|d| d.y == 0.0));
        assert_eq!(
            ParallaxLayer::default()
                .copies(DVec2::ZERO, 1.0, DVec2::ZERO, DVec2::new(1.0, 1.0))
                .len(),
            1
        );
    }
}
