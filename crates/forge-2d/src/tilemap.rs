//! Tilemaps with autotiling and layers (Ch.35 §35.2).
//!
//! A [`Tilemap`] is a grid of cells in layers, stored as 16x16 chunks so an endless map
//! costs only what is painted. A cell holds an explicit tile, an **autotile terrain**, or
//! nothing. A terrain cell's tile is solved from its 8 neighbours with the **blob** rules
//! (47 shapes: a corner counts only when both edges beside it are the same terrain), falling
//! back to the edges-only shape and then to the isolated tile, so a tile set drawn with 16
//! edge tiles works as well as a full 47-tile one. The editor's tile palette (WP-U13) asks
//! these same functions through `forge_editor::domain::scene2d::Scene2d`.
//!
//! Rendering: [`Tilemap::sprites`] resolves the chunks the camera sees into sprites (the
//! batcher draws a layer on one tile set in one call). Collision: [`Tilemap::solid_rects`]
//! merges solid cells into as few rectangles as it can (runs, then stacked runs) for the
//! physics world and the shadow occluders.

use std::collections::{BTreeMap, BTreeSet};

use forge_frames::{FrameId, FramePos2};
use forge_num::DVec2;

use crate::Error2d;
use crate::math::Aabb2;
use crate::sprite::{Sprite, TextureId, UvRect};

/// Cells per chunk side.
pub const CHUNK: i32 = 16;
const CELLS: usize = (CHUNK * CHUNK) as usize;

/// Neighbour bits of an autotile mask (clockwise from north).
pub const N: u8 = 1;
pub const NE: u8 = 2;
pub const E: u8 = 4;
pub const SE: u8 = 8;
pub const S: u8 = 16;
pub const SW: u8 = 32;
pub const W: u8 = 64;
pub const NW: u8 = 128;

/// `(bit, dx, dy)` with y growing downwards (row order, as a tile map is drawn).
pub const NEIGHBOURS: [(u8, i32, i32); 8] = [
    (N, 0, -1),
    (NE, 1, -1),
    (E, 1, 0),
    (SE, 1, 1),
    (S, 0, 1),
    (SW, -1, 1),
    (W, -1, 0),
    (NW, -1, -1),
];

/// The blob reduction: a corner counts only when both edges beside it are set (otherwise it
/// cannot show), folding the 256 masks onto the 47 a tile set draws.
#[must_use]
pub fn canonical(mask: u8) -> u8 {
    let mut m = mask & (N | E | S | W);
    for (corner, a, b) in [(NE, N, E), (SE, S, E), (SW, S, W), (NW, N, W)] {
        if mask & corner != 0 && mask & a != 0 && mask & b != 0 {
            m |= corner;
        }
    }
    m
}

/// The 47 canonical masks, ascending: the order a blob template lays its tiles in.
#[must_use]
pub fn blob47() -> Vec<u8> {
    let set: BTreeSet<u8> = (0..=255u8).map(canonical).collect();
    set.into_iter().collect()
}

/// The tile a terrain cell with raw neighbour mask `mask` shows under `rules` (canonical
/// mask -> tile): the exact shape, then its edges alone, then the isolated tile; `None`
/// when the terrain has none of those.
#[must_use]
pub fn autotile(rules: &BTreeMap<u8, u32>, mask: u8) -> Option<u32> {
    let c = canonical(mask);
    rules
        .get(&c)
        .or_else(|| rules.get(&(c & (N | E | S | W))))
        .or_else(|| rules.get(&0))
        .copied()
}

/// A tile set: the image grid and its terrains.
#[derive(Clone, Debug, PartialEq)]
pub struct Tileset {
    /// The texture holding the tiles, and where the grid starts in it (pixels).
    pub texture: TextureId,
    pub texture_size: (u32, u32),
    pub origin_px: (u32, u32),
    /// Tile size in pixels, and the grid's columns and tile count.
    pub tile_px: (u32, u32),
    pub columns: u32,
    pub count: u32,
    /// Terrain -> (canonical mask -> tile).
    pub terrains: Vec<BTreeMap<u8, u32>>,
    /// Tiles that collide and cast shadows.
    pub solid: BTreeSet<u32>,
}

impl Tileset {
    /// Texture coordinates of `tile`, or `None` past the set.
    #[must_use]
    pub fn uv(&self, tile: u32) -> Option<UvRect> {
        if tile >= self.count || self.columns == 0 {
            return None;
        }
        let (tw, th) = self.tile_px;
        let x = self.origin_px.0 + (tile % self.columns) * tw;
        let y = self.origin_px.1 + (tile / self.columns) * th;
        Some(UvRect::from_pixels(
            f64::from(x),
            f64::from(y),
            f64::from(tw),
            f64::from(th),
            self.texture_size.0,
            self.texture_size.1,
        ))
    }
}

/// One cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cell {
    #[default]
    Empty,
    Tile(u32),
    /// Index into the tile set's terrains.
    Terrain(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Chunk {
    cells: Vec<Cell>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            cells: vec![Cell::Empty; CELLS],
        }
    }
}

/// The chunk holding cell `(x, y)` and the cell's index in it.
#[must_use]
pub fn chunk_of(x: i32, y: i32) -> ((i32, i32), usize) {
    let (cx, cy) = (x.div_euclid(CHUNK), y.div_euclid(CHUNK));
    let (lx, ly) = (x.rem_euclid(CHUNK), y.rem_euclid(CHUNK));
    ((cx, cy), (ly * CHUNK + lx) as usize)
}

/// A layer of cells.
#[derive(Clone, Debug, PartialEq)]
pub struct TileLayer {
    pub name: String,
    pub visible: bool,
    /// The sprite layer it draws on (and its order inside it).
    pub draw_layer: i32,
    pub draw_order: i32,
    /// Solid cells of this layer collide and cast shadows.
    pub collides: bool,
    chunks: BTreeMap<(i32, i32), Chunk>,
}

/// See the module docs. Cell `(x, y)` covers world `[x, x+1) x [-(y+1), -y)` times
/// `cell_size`, from `origin` (rows grow downward, like the image of a level).
#[derive(Clone, Debug, PartialEq)]
pub struct Tilemap {
    pub origin: FramePos2,
    /// World size of a cell.
    pub cell_size: DVec2,
    pub tileset: Tileset,
    pub layers: Vec<TileLayer>,
}

impl Tilemap {
    /// An empty map.
    #[must_use]
    pub fn new(origin: FramePos2, cell_size: DVec2, tileset: Tileset) -> Self {
        Self {
            origin,
            cell_size,
            tileset,
            layers: Vec::new(),
        }
    }

    /// Add a layer; returns its index.
    pub fn add_layer(&mut self, name: &str, draw_layer: i32, collides: bool) -> usize {
        self.layers.push(TileLayer {
            name: name.to_string(),
            visible: true,
            draw_layer,
            draw_order: 0,
            collides,
            chunks: BTreeMap::new(),
        });
        self.layers.len() - 1
    }

    fn layer(&self, l: usize) -> Result<&TileLayer, Error2d> {
        self.layers
            .get(l)
            .ok_or_else(|| Error2d::invalid(format!("no tile layer {l}")))
    }

    /// Set a cell.
    pub fn set(&mut self, l: usize, x: i32, y: i32, cell: Cell) -> Result<(), Error2d> {
        if let Cell::Terrain(t) = cell
            && usize::from(t) >= self.tileset.terrains.len()
        {
            return Err(Error2d::invalid(format!("no terrain {t}")));
        }
        let layer = self
            .layers
            .get_mut(l)
            .ok_or_else(|| Error2d::invalid(format!("no tile layer {l}")))?;
        let (c, i) = chunk_of(x, y);
        let ch = layer.chunks.entry(c).or_default();
        ch.cells[i] = cell;
        if cell == Cell::Empty && ch.cells.iter().all(|c| *c == Cell::Empty) {
            layer.chunks.remove(&c);
        }
        Ok(())
    }

    /// A cell (empty outside every chunk).
    pub fn get(&self, l: usize, x: i32, y: i32) -> Result<Cell, Error2d> {
        let layer = self.layer(l)?;
        let (c, i) = chunk_of(x, y);
        Ok(layer.chunks.get(&c).map_or(Cell::Empty, |ch| ch.cells[i]))
    }

    fn cell(layer: &TileLayer, x: i32, y: i32) -> Cell {
        let (c, i) = chunk_of(x, y);
        layer.chunks.get(&c).map_or(Cell::Empty, |ch| ch.cells[i])
    }

    /// The neighbour mask of `(x, y)` for `terrain`.
    pub fn mask(&self, l: usize, x: i32, y: i32, terrain: u16) -> Result<u8, Error2d> {
        let layer = self.layer(l)?;
        Ok(Self::mask_in(layer, x, y, terrain))
    }

    fn mask_in(layer: &TileLayer, x: i32, y: i32, terrain: u16) -> u8 {
        let mut m = 0;
        for (bit, dx, dy) in NEIGHBOURS {
            if Self::cell(layer, x + dx, y + dy) == Cell::Terrain(terrain) {
                m |= bit;
            }
        }
        m
    }

    /// The tile cell `(x, y)` shows (terrains solved), or `None`.
    pub fn resolve(&self, l: usize, x: i32, y: i32) -> Result<Option<u32>, Error2d> {
        let layer = self.layer(l)?;
        Ok(self.resolve_in(layer, x, y))
    }

    fn resolve_in(&self, layer: &TileLayer, x: i32, y: i32) -> Option<u32> {
        match Self::cell(layer, x, y) {
            Cell::Empty => None,
            Cell::Tile(t) => Some(t),
            Cell::Terrain(t) => {
                let rules = self.tileset.terrains.get(usize::from(t))?;
                autotile(rules, Self::mask_in(layer, x, y, t))
            }
        }
    }

    /// The world box of cell `(x, y)` (offsets from `origin`).
    #[must_use]
    pub fn cell_box(&self, x: i32, y: i32) -> Aabb2 {
        let lo = DVec2::new(
            f64::from(x) * self.cell_size.x,
            -f64::from(y + 1) * self.cell_size.y,
        );
        Aabb2::new(lo, lo + self.cell_size)
    }

    /// The cell holding world point `p`.
    pub fn cell_at(&self, p: FramePos2) -> Result<(i32, i32), Error2d> {
        if p.frame != self.origin.frame {
            return Err(Error2d::Frame {
                why: format!(
                    "point in {:?}, tile map in {:?}",
                    p.frame, self.origin.frame
                ),
            });
        }
        let d = p.local - self.origin.local;
        Ok((
            (d.x / self.cell_size.x).floor() as i32,
            (-d.y / self.cell_size.y).floor() as i32,
        ))
    }

    /// Sprites for the visible cells of every visible layer inside `view` (world box,
    /// offsets from `origin`), appended to `out`.
    pub fn sprites(&self, view: &Aabb2, out: &mut Vec<Sprite>) {
        let (c0, c1) = self.chunk_range(view);
        for layer in self.layers.iter().filter(|l| l.visible) {
            for ((cx, cy), ch) in layer.chunks.range((c0.0, i32::MIN)..=(c1.0, i32::MAX)) {
                if *cy < c0.1 || *cy > c1.1 {
                    continue;
                }
                for (i, cell) in ch.cells.iter().enumerate() {
                    if *cell == Cell::Empty {
                        continue;
                    }
                    let x = cx * CHUNK + i as i32 % CHUNK;
                    let y = cy * CHUNK + i as i32 / CHUNK;
                    let b = self.cell_box(x, y);
                    if !b.overlaps(view) {
                        continue;
                    }
                    let Some(tile) = self.resolve_in(layer, x, y) else {
                        continue;
                    };
                    let Some(uv) = self.tileset.uv(tile) else {
                        continue;
                    };
                    out.push(Sprite {
                        pos: FramePos2::new(self.origin.frame, self.origin.local + b.center()),
                        size: self.cell_size,
                        pivot: DVec2::new(0.5, 0.5),
                        angle: 0.0,
                        uv,
                        texture: self.tileset.texture,
                        tint: [1.0; 4],
                        layer: layer.draw_layer,
                        order: layer.draw_order,
                        flip_x: false,
                        flip_y: false,
                    });
                }
            }
        }
    }

    /// The chunk coordinates `view` touches: `((cx0, cy0), (cx1, cy1))`.
    fn chunk_range(&self, view: &Aabb2) -> ((i32, i32), (i32, i32)) {
        let fx = |v: f64| (v / self.cell_size.x).floor() as i32;
        let fy = |v: f64| (-v / self.cell_size.y).floor() as i32;
        let (x0, x1) = (fx(view.min.x), fx(view.max.x));
        let (y0, y1) = (fy(view.max.y), fy(view.min.y));
        (
            (x0.div_euclid(CHUNK), y0.div_euclid(CHUNK)),
            (x1.div_euclid(CHUNK), y1.div_euclid(CHUNK)),
        )
    }

    /// Is the cell solid in any colliding layer (an explicit solid tile, or a terrain whose
    /// resolved tile is solid)?
    #[must_use]
    pub fn is_solid(&self, x: i32, y: i32) -> bool {
        self.layers.iter().filter(|l| l.collides).any(|l| {
            self.resolve_in(l, x, y)
                .is_some_and(|t| self.tileset.solid.contains(&t))
        })
    }

    /// Every solid cell of the colliding layers, merged into rectangles `(x, y, w, h)` in
    /// cells: horizontal runs first, then runs of equal extent stacked downward. Sorted.
    #[must_use]
    pub fn solid_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        let mut cells: BTreeSet<(i32, i32)> = BTreeSet::new();
        for l in self.layers.iter().filter(|l| l.collides) {
            for ((cx, cy), ch) in &l.chunks {
                for (i, c) in ch.cells.iter().enumerate() {
                    if *c == Cell::Empty {
                        continue;
                    }
                    let x = cx * CHUNK + i as i32 % CHUNK;
                    let y = cy * CHUNK + i as i32 / CHUNK;
                    if self.is_solid(x, y) {
                        // Row-major: (y, x).
                        cells.insert((y, x));
                    }
                }
            }
        }
        // Runs per row.
        let mut runs: BTreeMap<(i32, i32, i32), ()> = BTreeMap::new(); // (x, w, y)
        let mut it = cells.iter().peekable();
        while let Some(&(y, x)) = it.next() {
            let mut w = 1;
            while it.peek() == Some(&&(y, x + w)) {
                it.next();
                w += 1;
            }
            runs.insert((x, w, y), ());
        }
        // Stack equal runs on consecutive rows.
        let mut out = Vec::new();
        let keys: Vec<(i32, i32, i32)> = runs.keys().copied().collect();
        let mut used: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
        for (x, w, y) in keys {
            if used.contains(&(x, w, y)) {
                continue;
            }
            let mut h = 1;
            while runs.contains_key(&(x, w, y + h)) && !used.contains(&(x, w, y + h)) {
                used.insert((x, w, y + h));
                h += 1;
            }
            used.insert((x, w, y));
            out.push((x, y, w, h));
        }
        out.sort_unstable();
        out
    }

    /// [`Self::solid_rects`] as world boxes (offsets from `origin`).
    #[must_use]
    pub fn solid_boxes(&self) -> Vec<Aabb2> {
        self.solid_rects()
            .into_iter()
            .map(|(x, y, w, h)| {
                let lo = DVec2::new(
                    f64::from(x) * self.cell_size.x,
                    -f64::from(y + h) * self.cell_size.y,
                );
                Aabb2::new(
                    lo,
                    lo + DVec2::new(
                        f64::from(w) * self.cell_size.x,
                        f64::from(h) * self.cell_size.y,
                    ),
                )
            })
            .collect()
    }

    /// Painted cells across all layers.
    #[must_use]
    pub fn painted(&self) -> usize {
        self.layers
            .iter()
            .flat_map(|l| l.chunks.values())
            .map(|c| c.cells.iter().filter(|x| **x != Cell::Empty).count())
            .sum()
    }

    /// The frame the map is in.
    #[must_use]
    pub fn frame(&self) -> FrameId {
        self.origin.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tileset() -> Tileset {
        let mut rules = BTreeMap::new();
        for (i, m) in blob47().into_iter().enumerate() {
            rules.insert(m, 100 + i as u32);
        }
        Tileset {
            texture: TextureId(1),
            texture_size: (256, 256),
            origin_px: (0, 0),
            tile_px: (16, 16),
            columns: 16,
            count: 256,
            terrains: vec![rules],
            solid: (100..147).chain([7]).collect(),
        }
    }

    fn map() -> Tilemap {
        let mut m = Tilemap::new(
            FramePos2::new(FrameId(0), DVec2::ZERO),
            DVec2::new(1.0, 1.0),
            tileset(),
        );
        m.add_layer("ground", 0, true);
        m
    }

    #[test]
    fn the_blob_has_47_shapes_and_corners_need_both_edges() {
        assert_eq!(blob47().len(), 47);
        assert_eq!(canonical(NE), 0);
        assert_eq!(canonical(N | E | NE), N | E | NE);
        assert_eq!(canonical(0xFF), 0xFF);
    }

    #[test]
    fn terrain_cells_retile_from_their_neighbours_across_chunks() {
        let mut m = map();
        // A 3x3 block straddling the chunk corner at (16, 16).
        for y in 15..18 {
            for x in 15..18 {
                m.set(0, x, y, Cell::Terrain(0)).expect("set");
            }
        }
        let blob = blob47();
        let tile_of = |mask: u8| 100 + blob.iter().position(|b| *b == mask).expect("mask") as u32;
        assert_eq!(
            m.resolve(0, 16, 16).expect("r"),
            Some(tile_of(0xFF)),
            "centre: all neighbours"
        );
        assert_eq!(
            m.resolve(0, 15, 15).expect("r"),
            Some(tile_of(E | SE | S)),
            "top-left corner"
        );
        // Paint one more cell: its neighbour re-tiles without being written.
        m.set(0, 18, 16, Cell::Terrain(0)).expect("set");
        assert_eq!(
            m.resolve(0, 17, 16).expect("r"),
            Some(tile_of(canonical(N | NW | W | SW | S | E)))
        );
    }

    #[test]
    fn edges_only_and_isolated_fallbacks() {
        let mut rules = BTreeMap::new();
        rules.insert(0, 1);
        rules.insert(N | S, 2);
        assert_eq!(autotile(&rules, N | S | NE), Some(2));
        assert_eq!(autotile(&rules, N | E), Some(1));
        assert_eq!(autotile(&BTreeMap::new(), 0), None);
    }

    #[test]
    fn solid_cells_merge_into_few_rectangles_that_cover_them_exactly() {
        let mut m = map();
        // A floor 40 wide, 2 deep, and a pillar.
        for x in 0..40 {
            for y in 10..12 {
                m.set(0, x, y, Cell::Tile(7)).expect("set");
            }
        }
        for y in 5..10 {
            m.set(0, 20, y, Cell::Tile(7)).expect("set");
        }
        m.set(0, 30, 3, Cell::Tile(3)).expect("non-solid tile");
        let rects = m.solid_rects();
        assert!(rects.len() <= 3, "{rects:?}");
        let mut covered = BTreeSet::new();
        for (x, y, w, h) in &rects {
            for yy in *y..y + h {
                for xx in *x..x + w {
                    assert!(covered.insert((xx, yy)), "overlap at {xx},{yy}");
                }
            }
        }
        assert_eq!(covered.len(), 85);
        assert!(!m.is_solid(30, 3));
    }

    #[test]
    fn only_visible_cells_become_sprites() {
        let mut m = map();
        for x in 0..1000 {
            m.set(0, x, 0, Cell::Tile(7)).expect("set");
        }
        let mut out = Vec::new();
        m.sprites(
            &Aabb2::new(DVec2::new(100.0, -2.0), DVec2::new(120.0, 2.0)),
            &mut out,
        );
        assert!((20..=22).contains(&out.len()), "{}", out.len());
        assert!(out.iter().all(|s| s.texture == TextureId(1)));
        let (x, y) = m
            .cell_at(FramePos2::new(FrameId(0), DVec2::new(3.5, -0.5)))
            .expect("cell");
        assert_eq!((x, y), (3, 0));
    }
}
