//! 2D navigation (Ch.35 §35.2).
//!
//! A [`NavGrid`] marks which cells of a grid can be walked — built straight from a tile
//! map's solid cells ([`NavGrid::from_tilemap`]), optionally grown by an agent's radius —
//! and [`NavGrid::find_path`] runs A* over it (8-connected, no corner cutting, octile
//! distance), then pulls the path taut with line-of-sight checks so an agent walks straight
//! lines, not a staircase. Ties are broken by a fixed key, so the same query always gives
//! the same path (the I2 corpus hashes paths, `2d/nav/*`).

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use forge_frames::FramePos2;
use forge_num::DVec2;

use crate::Error2d;
use crate::tilemap::Tilemap;

/// See the module docs. Cell `(x, y)` covers `origin + [x, x+1) x [-(y+1), -y)` cells
/// (rows down, like a tile map).
#[derive(Clone, Debug, PartialEq)]
pub struct NavGrid {
    pub origin: FramePos2,
    pub cell: DVec2,
    /// The grid's first cell (it may start at negative tile coordinates).
    pub x0: i32,
    pub y0: i32,
    pub w: u32,
    pub h: u32,
    walk: Vec<bool>,
}

#[derive(Clone, Copy, PartialEq)]
struct Open {
    f: f64,
    h: f64,
    idx: u32,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on (f, h, idx).
        o.f.total_cmp(&self.f)
            .then(o.h.total_cmp(&self.h))
            .then(o.idx.cmp(&self.idx))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

const SQRT2: f64 = core::f64::consts::SQRT_2;

impl NavGrid {
    /// A grid of `w x h` cells starting at cell `(x0, y0)`, all walkable.
    #[must_use]
    pub fn open(origin: FramePos2, cell: DVec2, x0: i32, y0: i32, w: u32, h: u32) -> Self {
        Self {
            origin,
            cell,
            x0,
            y0,
            w,
            h,
            walk: vec![true; (w as usize) * (h as usize)],
        }
    }

    /// The walkable cells of `map` inside `(x0, y0, w, h)`: not solid, and (with
    /// `clearance` cells) not within that many cells of a solid one.
    #[must_use]
    pub fn from_tilemap(map: &Tilemap, x0: i32, y0: i32, w: u32, h: u32, clearance: u32) -> Self {
        let mut g = Self::open(map.origin, map.cell_size, x0, y0, w, h);
        let c = clearance as i32;
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let (tx, ty) = (x0 + x, y0 + y);
                let blocked = (-c..=c).any(|dy| (-c..=c).any(|dx| map.is_solid(tx + dx, ty + dy)));
                g.walk[(y as u32 * w + x as u32) as usize] = !blocked;
            }
        }
        g
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let (lx, ly) = (x - self.x0, y - self.y0);
        (lx >= 0 && ly >= 0 && (lx as u32) < self.w && (ly as u32) < self.h)
            .then(|| (ly as u32 * self.w + lx as u32) as usize)
    }

    /// Can cell `(x, y)` be walked (outside the grid: no)?
    #[must_use]
    pub fn walkable(&self, x: i32, y: i32) -> bool {
        self.index(x, y).is_some_and(|i| self.walk[i])
    }

    /// Block or open a cell.
    pub fn set(&mut self, x: i32, y: i32, walkable: bool) {
        if let Some(i) = self.index(x, y) {
            self.walk[i] = walkable;
        }
    }

    /// The cell holding `p`.
    pub fn cell_at(&self, p: FramePos2) -> Result<(i32, i32), Error2d> {
        if p.frame != self.origin.frame {
            return Err(Error2d::Frame {
                why: format!("point in {:?}, grid in {:?}", p.frame, self.origin.frame),
            });
        }
        let d = p.local - self.origin.local;
        Ok((
            (d.x / self.cell.x).floor() as i32,
            (-d.y / self.cell.y).floor() as i32,
        ))
    }

    /// The centre of cell `(x, y)`.
    #[must_use]
    pub fn center_of(&self, x: i32, y: i32) -> FramePos2 {
        FramePos2::new(
            self.origin.frame,
            self.origin.local
                + DVec2::new(
                    (f64::from(x) + 0.5) * self.cell.x,
                    -(f64::from(y) + 0.5) * self.cell.y,
                ),
        )
    }

    fn octile(a: (i32, i32), b: (i32, i32)) -> f64 {
        let dx = f64::from((a.0 - b.0).abs());
        let dy = f64::from((a.1 - b.1).abs());
        dx.max(dy) + (SQRT2 - 1.0) * dx.min(dy)
    }

    /// The cells from `start` to `goal` (both included), or `None` when the goal cannot be
    /// reached. Errors: a point in another frame, or a start or goal off the walkable grid.
    pub fn find_cells(
        &self,
        start: (i32, i32),
        goal: (i32, i32),
    ) -> Result<Option<Vec<(i32, i32)>>, Error2d> {
        let (Some(si), Some(gi)) = (self.index(start.0, start.1), self.index(goal.0, goal.1))
        else {
            return Err(Error2d::invalid(
                "the start or goal is off the navigation grid",
            ));
        };
        if !self.walk[si] || !self.walk[gi] {
            return Ok(None);
        }
        let n = self.walk.len();
        let mut g = vec![f64::INFINITY; n];
        let mut came = vec![u32::MAX; n];
        let mut closed = vec![false; n];
        let mut open = BinaryHeap::new();
        g[si] = 0.0;
        let h0 = Self::octile(start, goal);
        open.push(Open {
            f: h0,
            h: h0,
            idx: si as u32,
        });
        let w = self.w as i32;
        while let Some(Open { idx, .. }) = open.pop() {
            let i = idx as usize;
            if closed[i] {
                continue;
            }
            if i == gi {
                let mut path = Vec::new();
                let mut cur = i;
                loop {
                    path.push((self.x0 + cur as i32 % w, self.y0 + cur as i32 / w));
                    if cur == si {
                        break;
                    }
                    cur = came[cur] as usize;
                }
                path.reverse();
                return Ok(Some(path));
            }
            closed[i] = true;
            let (cx, cy) = (self.x0 + i as i32 % w, self.y0 + i as i32 / w);
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let (nx, ny) = (cx + dx, cy + dy);
                let Some(ni) = self.index(nx, ny) else {
                    continue;
                };
                if !self.walk[ni] || closed[ni] {
                    continue;
                }
                if dx != 0 && dy != 0 && !(self.walkable(cx + dx, cy) && self.walkable(cx, cy + dy))
                {
                    continue; // no corner cutting
                }
                let step = if dx != 0 && dy != 0 { SQRT2 } else { 1.0 };
                let ng = g[i] + step;
                if ng < g[ni] {
                    g[ni] = ng;
                    came[ni] = i as u32;
                    let h = Self::octile((nx, ny), goal);
                    open.push(Open {
                        f: ng + h,
                        h,
                        idx: ni as u32,
                    });
                }
            }
        }
        Ok(None)
    }

    /// Is the straight line between two cell centres clear (every cell it touches
    /// walkable, corners included)?
    #[must_use]
    pub fn line_of_sight(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        // Supercover traversal (Amanatides-Woo) in cell units, centre to centre.
        let (mut x, mut y) = a;
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let (sx, sy) = (dx.signum(), dy.signum());
        let (nx, ny) = (dx.abs(), dy.abs());
        let (mut ix, mut iy) = (0, 0);
        if !self.walkable(x, y) {
            return false;
        }
        while ix < nx || iy < ny {
            // Compare (0.5 + ix) / nx with (0.5 + iy) / ny in integers.
            let lhs = (1 + 2 * ix) * ny;
            let rhs = (1 + 2 * iy) * nx;
            if lhs == rhs {
                // Through a corner: both side cells must be open.
                if !self.walkable(x + sx, y) || !self.walkable(x, y + sy) {
                    return false;
                }
                x += sx;
                y += sy;
                ix += 1;
                iy += 1;
            } else if lhs < rhs {
                x += sx;
                ix += 1;
            } else {
                y += sy;
                iy += 1;
            }
            if !self.walkable(x, y) {
                return false;
            }
        }
        true
    }

    /// A path from `start` to `goal` as waypoints (cell centres, the ends exactly the
    /// given points), pulled taut by line of sight. `None`: unreachable.
    pub fn find_path(
        &self,
        start: FramePos2,
        goal: FramePos2,
    ) -> Result<Option<Vec<FramePos2>>, Error2d> {
        let (s, g) = (self.cell_at(start)?, self.cell_at(goal)?);
        let Some(cells) = self.find_cells(s, g)? else {
            return Ok(None);
        };
        let mut keep = vec![cells[0]];
        let mut anchor = 0;
        for i in 1..cells.len() {
            if !self.line_of_sight(cells[anchor], cells[i]) {
                anchor = i - 1;
                keep.push(cells[anchor]);
            }
        }
        if keep.last() != cells.last() {
            keep.push(cells[cells.len() - 1]);
        }
        let mut out: Vec<FramePos2> = keep.iter().map(|c| self.center_of(c.0, c.1)).collect();
        if let Some(f) = out.first_mut() {
            *f = start;
        }
        if let Some(l) = out.last_mut() {
            *l = goal;
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::FrameId;

    fn grid() -> NavGrid {
        let mut g = NavGrid::open(
            FramePos2::new(FrameId(0), DVec2::ZERO),
            DVec2::new(1.0, 1.0),
            0,
            0,
            20,
            12,
        );
        // A wall at x = 10 with a gap at y = 9.
        for y in 0..12 {
            if y != 9 {
                g.set(10, y, false);
            }
        }
        g
    }

    #[test]
    fn a_star_finds_the_shortest_way_round_and_through_the_gap() {
        let g = grid();
        let cells = g.find_cells((2, 2), (17, 2)).expect("q").expect("path");
        assert!(cells.contains(&(10, 9)), "through the gap");
        let len: f64 = cells
            .windows(2)
            .map(|w| {
                if w[0].0 != w[1].0 && w[0].1 != w[1].1 {
                    SQRT2
                } else {
                    1.0
                }
            })
            .sum();
        // Octile optimum without cutting the wall's corners: (2,2) to (9,9) is 7 diagonals,
        // straight through the gap to (11,9), then 6 diagonals and 1 straight to (17,2).
        let optimum = 7.0 * SQRT2 + 2.0 + 6.0 * SQRT2 + 1.0;
        assert!((len - optimum).abs() < 1e-9, "{len} vs {optimum}");
        assert_eq!(
            g.find_cells((2, 2), (17, 2)).expect("q"),
            Some(cells),
            "deterministic"
        );
    }

    #[test]
    fn no_corner_cutting_and_unreachable_is_none() {
        let mut g = NavGrid::open(
            FramePos2::new(FrameId(0), DVec2::ZERO),
            DVec2::new(1.0, 1.0),
            0,
            0,
            3,
            3,
        );
        g.set(1, 0, false);
        g.set(0, 1, false);
        assert_eq!(
            g.find_cells((0, 0), (1, 1)).expect("q"),
            None,
            "diagonal through a corner is refused"
        );
        assert!(g.find_cells((0, 0), (9, 9)).is_err());
    }

    #[test]
    fn paths_are_pulled_taut() {
        let g = grid();
        let p = g
            .find_path(
                FramePos2::new(FrameId(0), DVec2::new(2.5, -2.5)),
                FramePos2::new(FrameId(0), DVec2::new(17.5, -2.5)),
            )
            .expect("q")
            .expect("path");
        assert!(p.len() <= 4, "straight lines to the gap and away: {p:?}");
        for w in p.windows(2) {
            let a = g.cell_at(w[0]).expect("cell");
            let b = g.cell_at(w[1]).expect("cell");
            assert!(g.line_of_sight(a, b));
        }
        assert!(!g.line_of_sight((2, 2), (17, 2)));
    }
}
