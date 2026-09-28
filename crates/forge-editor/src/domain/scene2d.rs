//! The 2D editors' model (Ch.35 §35.2, Ch.21 §21.21 "2D editors", DoD M2-66): tile sets
//! with autotile terrains, layered tile maps, sprite sheets sliced into frames with frame
//! animations, and `Skeleton2D` cutout rigs.
//!
//! Project settings (see [`super`]; `<…>` are identifiers, `<n>` decimal indices):
//!
//! | Key | Value |
//! |---|---|
//! | `d2.tileset.<ts>.{name,image}` | text |
//! | `d2.tileset.<ts>.{tile_w,tile_h,columns,count}` | integer (px, tiles) |
//! | `d2.tileset.<ts>.terrain.<tr>.name` | text |
//! | `d2.tileset.<ts>.terrain.<tr>.rule.<mask>` | integer: the tile a cell with that (canonical) neighbour mask shows |
//! | `d2.tilemap.<tm>.{name,tileset}` | text |
//! | `d2.tilemap.<tm>.layer.<n>.{name,visible}` | text, bool |
//! | `d2.tilemap.<tm>.layer.<n>.chunk.<c>` | text: 16×16 cells, row-major, `,`-separated: empty, `t<tile>` or `a<terrain>` |
//! | `d2.sheet.<sh>.{name,image}` | text |
//! | `d2.sheet.<sh>.{image_w,image_h,cell_w,cell_h,offset_x,offset_y,spacing_x,spacing_y}` | integer (px) |
//! | `d2.sheet.<sh>.anim.<an>.{name,frames,fps,looping}` | text, text (`0,1,2`), number, bool |
//! | `d2.rig.<rg>.name` | text |
//! | `d2.rig.<rg>.bone.<b>.{name,parent,sprite}` | text (`parent` empty: a root) |
//! | `d2.rig.<rg>.bone.<b>.{x,y,rot,len}` | number (px, degrees, px), in the parent bone's frame |
//! | `d2.rig.<rg>.bone.<b>.z` | integer (draw order) |
//!
//! A painted cell stores its **terrain**, not the tile it shows: the tile is solved from
//! the 8 neighbours when shown ([`Scene2d::autotile`]), so painting one cell re-tiles its
//! neighbours without writing them — a stroke writes only the chunks it touched.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use forge_cmd::{EditorCommand, Value};

use super::{BackendInfo, float, int, objects, set, sub_objects, text};
use crate::mirror::ProjectMirror;

pub const TILESET: &str = "d2.tileset";
pub const TILEMAP: &str = "d2.tilemap";
pub const SHEET: &str = "d2.sheet";
pub const RIG: &str = "d2.rig";
/// Cells per chunk side.
pub const CHUNK: i64 = 16;
const CELLS: usize = (CHUNK * CHUNK) as usize;

// ---- tile sets and autotiling -----------------------------------------------------------

/// Neighbour bits of an autotile mask (clockwise from north).
pub const N: u8 = 1;
pub const NE: u8 = 2;
pub const E: u8 = 4;
pub const SE: u8 = 8;
pub const S: u8 = 16;
pub const SW: u8 = 32;
pub const W: u8 = 64;
pub const NW: u8 = 128;

/// `(bit, dx, dy)` with y growing downwards (row order).
pub const NEIGHBOURS: [(u8, i64, i64); 8] = [
    (N, 0, -1),
    (NE, 1, -1),
    (E, 1, 0),
    (SE, 1, 1),
    (S, 0, 1),
    (SW, -1, 1),
    (W, -1, 0),
    (NW, -1, -1),
];

/// The blob reduction: a corner counts only when both edges beside it are the same terrain
/// (otherwise it cannot show), which folds the 256 masks onto the 47 a tile set draws.
/// The 2D pipeline's own rule (`forge_2d::tilemap::canonical`): one definition for the
/// editor and the game.
pub fn canonical(mask: u8) -> u8 {
    forge_2d::tilemap::canonical(mask)
}

/// The 47 canonical masks, ascending: the order a "blob" template lays its tiles in.
pub fn blob47() -> Vec<u8> {
    forge_2d::tilemap::blob47()
}

/// An autotile terrain: its rules map a canonical mask to a tile.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Terrain {
    pub id: String,
    pub name: String,
    pub rules: BTreeMap<u8, u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tileset {
    pub id: String,
    pub name: String,
    /// The source image (an asset path).
    pub image: String,
    pub tile_w: i64,
    pub tile_h: i64,
    pub columns: i64,
    pub count: i64,
    pub terrains: BTreeMap<String, Terrain>,
}

// ---- tile maps --------------------------------------------------------------------------

/// One map cell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Cell {
    #[default]
    Empty,
    /// An explicit tile.
    Tile(u32),
    /// An autotiled terrain (the tile is solved from the neighbours).
    Terrain(String),
}

impl Cell {
    fn token(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Tile(t) => format!("t{t}"),
            Cell::Terrain(t) => format!("a{t}"),
        }
    }
    fn parse(tok: &str) -> Option<Cell> {
        if tok.is_empty() {
            return Some(Cell::Empty);
        }
        let (k, rest) = tok.split_at(1);
        match k {
            "t" => rest.parse().ok().map(Cell::Tile),
            "a" if super::is_ident(rest) => Some(Cell::Terrain(rest.to_string())),
            _ => None,
        }
    }
}

/// 16×16 cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub cells: Vec<Cell>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            cells: vec![Cell::Empty; CELLS],
        }
    }
}

impl Chunk {
    pub fn parse(text: &str) -> Result<Chunk, String> {
        let toks: Vec<&str> = text.split(',').collect();
        if toks.len() != CELLS {
            return Err(format!("{} cells, expected {CELLS}", toks.len()));
        }
        let cells = toks
            .iter()
            .map(|t| Cell::parse(t).ok_or_else(|| format!("bad cell {t:?}")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Chunk { cells })
    }
    pub fn encode(&self) -> String {
        self.cells
            .iter()
            .map(Cell::token)
            .collect::<Vec<_>>()
            .join(",")
    }
    pub fn is_empty(&self) -> bool {
        self.cells.iter().all(|c| *c == Cell::Empty)
    }
}

/// The chunk holding cell `(x, y)` and the cell's index in it.
pub fn chunk_of(x: i64, y: i64) -> ((i64, i64), usize) {
    let (cx, cy) = (x.div_euclid(CHUNK), y.div_euclid(CHUNK));
    let (lx, ly) = (x.rem_euclid(CHUNK), y.rem_euclid(CHUNK));
    ((cx, cy), (ly * CHUNK + lx) as usize)
}

/// A chunk coordinate as a key segment: `c3_m2` is chunk (3, −2).
pub fn chunk_segment((cx, cy): (i64, i64)) -> String {
    let part = |v: i64| {
        if v < 0 {
            format!("m{}", v.unsigned_abs())
        } else {
            v.to_string()
        }
    };
    format!("c{}_{}", part(cx), part(cy))
}

pub fn parse_chunk_segment(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.strip_prefix('c')?.split_once('_')?;
    let part = |p: &str| -> Option<i64> {
        match p.strip_prefix('m') {
            Some(n) => n.parse::<i64>().ok().map(|v| -v),
            None => p.parse().ok(),
        }
    };
    Some((part(a)?, part(b)?))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub index: u32,
    pub name: String,
    pub visible: bool,
    pub chunks: BTreeMap<(i64, i64), Chunk>,
}

impl Layer {
    pub fn cell(&self, x: i64, y: i64) -> &Cell {
        static EMPTY: Cell = Cell::Empty;
        let (c, i) = chunk_of(x, y);
        self.chunks
            .get(&c)
            .and_then(|ch| ch.cells.get(i))
            .unwrap_or(&EMPTY)
    }
    /// The neighbour mask of `(x, y)` for `terrain`, read through `over` first (a stroke
    /// in progress) and then this layer.
    pub fn mask(&self, over: &Stroke, x: i64, y: i64, terrain: &str) -> u8 {
        let mut m = 0;
        for (bit, dx, dy) in NEIGHBOURS {
            if matches!(over.cell(self, x + dx, y + dy), Cell::Terrain(t) if t == terrain) {
                m |= bit;
            }
        }
        m
    }
    /// Painted cells (the bounds of what is drawn), `None` when empty.
    pub fn bounds(&self) -> Option<(i64, i64, i64, i64)> {
        let mut b: Option<(i64, i64, i64, i64)> = None;
        for ((cx, cy), ch) in &self.chunks {
            for (i, c) in ch.cells.iter().enumerate() {
                if *c == Cell::Empty {
                    continue;
                }
                let x = cx * CHUNK + i as i64 % CHUNK;
                let y = cy * CHUNK + i as i64 / CHUNK;
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
        b
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tilemap {
    pub id: String,
    pub name: String,
    pub tileset: String,
    /// Shared, so the canvas shows a layer without copying it and an edit to one chunk
    /// copies nothing else ([`Doc2d::apply_setting`]).
    pub layers: BTreeMap<u32, Arc<Layer>>,
}

/// A paint stroke in progress: the chunks it changed, as they will be written, and which
/// of them changed since the last step that was sent. A step sends only those
/// ([`Stroke::unsent_commands`]): a gesture keeps only the newest step of a frame, so a
/// step replacing one still held back carries that one's chunks too (they are still
/// unsent), and nothing is lost — while a step after a sent one carries just the chunks
/// it dirtied, so a long stroke costs each step its own cells, not the stroke so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stroke {
    pub chunks: BTreeMap<(i64, i64), Chunk>,
    /// Chunks changed since the last step known to be sent.
    pub unsent: BTreeSet<(i64, i64)>,
}

impl Stroke {
    /// The cell as the stroke has it (falling back to the layer).
    pub fn cell<'a>(&'a self, layer: &'a Layer, x: i64, y: i64) -> &'a Cell {
        let (c, i) = chunk_of(x, y);
        match self.chunks.get(&c) {
            Some(ch) => ch.cells.get(i).unwrap_or(&Cell::Empty),
            None => layer.cell(x, y),
        }
    }
    /// Paint `cell` at `(x, y)`; `false` if it already was that.
    pub fn paint(&mut self, layer: &Layer, x: i64, y: i64, cell: Cell) -> bool {
        if *self.cell(layer, x, y) == cell {
            return false;
        }
        let (c, i) = chunk_of(x, y);
        let ch = self
            .chunks
            .entry(c)
            .or_insert_with(|| layer.chunks.get(&c).cloned().unwrap_or_default());
        if let Some(slot) = ch.cells.get_mut(i) {
            *slot = cell;
        }
        self.unsent.insert(c);
        true
    }
    fn command(map: &str, layer: u32, c: (i64, i64), ch: &Chunk) -> EditorCommand {
        let key = chunk_key(map, layer, c);
        if ch.is_empty() {
            super::clear(key)
        } else {
            set(key, Value::Text(ch.encode()))
        }
    }
    /// The commands writing the whole stroke (an emptied chunk clears its key).
    pub fn commands(&self, map: &str, layer: u32) -> Vec<EditorCommand> {
        self.chunks
            .iter()
            .map(|(c, ch)| Self::command(map, layer, *c, ch))
            .collect()
    }
    /// The commands writing the chunks changed since the last sent step.
    pub fn unsent_commands(&self, map: &str, layer: u32) -> Vec<EditorCommand> {
        self.unsent
            .iter()
            .filter_map(|c| {
                self.chunks
                    .get(c)
                    .map(|ch| Self::command(map, layer, *c, ch))
            })
            .collect()
    }
    /// Every step given so far has been sent: the next step starts from here.
    pub fn mark_sent(&mut self) {
        self.unsent.clear();
    }
}

pub fn chunk_key(map: &str, layer: u32, c: (i64, i64)) -> String {
    format!("{TILEMAP}.{map}.layer.{layer}.chunk.{}", chunk_segment(c))
}

// ---- sprite sheets ----------------------------------------------------------------------

/// How a sheet is cut into frames: a grid of `cell_w × cell_h` cells starting at the
/// offset, `spacing` apart, within the image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SheetSlice {
    pub image_w: i64,
    pub image_h: i64,
    pub cell_w: i64,
    pub cell_h: i64,
    pub offset_x: i64,
    pub offset_y: i64,
    pub spacing_x: i64,
    pub spacing_y: i64,
}

/// One frame of a sliced sheet, in image pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRect {
    pub index: u32,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Anim {
    pub id: String,
    pub name: String,
    pub frames: Vec<u32>,
    pub fps: f64,
    pub looping: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpriteSheet {
    pub id: String,
    pub name: String,
    pub image: String,
    pub slice: SheetSlice,
    pub anims: BTreeMap<String, Anim>,
}

pub fn parse_frames(s: &str) -> Result<Vec<u32>, String> {
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    s.split(',')
        .map(|t| {
            t.trim()
                .parse::<u32>()
                .map_err(|_| format!("bad frame {t:?}"))
        })
        .collect()
}

pub fn format_frames(f: &[u32]) -> String {
    f.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

// ---- Skeleton2D rigs --------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bone {
    pub id: String,
    pub name: String,
    /// `None`: a root bone.
    pub parent: Option<String>,
    /// Offset from the parent bone's origin, in the parent's rotated frame (px).
    pub x: f64,
    pub y: f64,
    /// Rotation relative to the parent (degrees, counter-clockwise).
    pub rot: f64,
    pub len: f64,
    /// The sprite drawn on the bone: `sheet:frame`, or empty.
    pub sprite: String,
    pub z: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rig2d {
    pub id: String,
    pub name: String,
    pub bones: BTreeMap<String, Bone>,
}

impl Rig2d {
    /// Would making `new_parent` the parent of `bone` close a loop?
    pub fn would_cycle(&self, bone: &str, new_parent: Option<&str>) -> bool {
        let mut cur = new_parent.map(str::to_string);
        let mut steps = 0;
        while let Some(p) = cur {
            if p == bone || steps > self.bones.len() {
                return true;
            }
            steps += 1;
            cur = self.bones.get(&p).and_then(|b| b.parent.clone());
        }
        false
    }
    /// `bone` and every bone under it.
    pub fn subtree(&self, bone: &str) -> Vec<String> {
        let mut out = vec![bone.to_string()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            for b in self.bones.values() {
                if b.parent.as_deref() == Some(cur.as_str()) && !out.contains(&b.id) {
                    out.push(b.id.clone());
                }
            }
            i += 1;
        }
        out
    }
}

/// A posed bone (rig space, px, y up).
#[derive(Clone, Debug, PartialEq)]
pub struct BonePose {
    pub id: String,
    pub origin: (f64, f64),
    pub tip: (f64, f64),
    /// World rotation (degrees).
    pub angle: f64,
}

/// A rig's pose: the bones that could be posed, and those that could not (a parent
/// missing, or a loop an automation session wrote), named.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pose {
    pub bones: Vec<BonePose>,
    pub broken: Vec<String>,
}

// ---- the document -----------------------------------------------------------------------

/// Everything 2D in the project, read from the mirror.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc2d {
    pub tilesets: BTreeMap<String, Tileset>,
    pub tilemaps: BTreeMap<String, Tilemap>,
    pub sheets: BTreeMap<String, SpriteSheet>,
    pub rigs: BTreeMap<String, Rig2d>,
    /// Values that could not be read (skipped), for the panel's warning line.
    pub problems: Vec<String>,
    /// Some layer or chunk key is not spelled canonically (`layer.01`, `chunk.c01_2`), so
    /// two keys may name one chunk: [`Doc2d::apply_setting`] then defers to a full read.
    pub aliased_chunks: bool,
}

impl Doc2d {
    pub fn read(m: &ProjectMirror) -> Doc2d {
        let mut d = Doc2d::default();
        for (id, f) in objects(m, TILESET) {
            let t = read_tileset(id, &f, &mut d.problems);
            d.tilesets.insert(id.to_string(), t);
        }
        for (id, f) in objects(m, TILEMAP) {
            let t = read_tilemap(id, &f, &mut d.problems, &mut d.aliased_chunks);
            d.tilemaps.insert(id.to_string(), t);
        }
        for (id, f) in objects(m, SHEET) {
            let s = read_sheet(id, &f, &mut d.problems);
            d.sheets.insert(id.to_string(), s);
        }
        for (id, f) in objects(m, RIG) {
            let r = read_rig(id, &f, &mut d.problems);
            d.rigs.insert(id.to_string(), r);
        }
        d
    }

    /// Is `key` a tile-map chunk (the only key a paint stroke writes)?
    pub fn is_chunk_key(key: &str) -> bool {
        key.strip_prefix(TILEMAP)
            .and_then(|r| r.strip_prefix('.'))
            .and_then(|r| r.split_once('.'))
            .and_then(|(_, r)| r.strip_prefix("layer."))
            .and_then(|r| r.split_once('.'))
            .is_some_and(|(_, f)| f.starts_with("chunk."))
    }

    /// Bring the document up to date with one changed setting `key` without re-reading
    /// the namespace: `Some(work)` if it could — the chunks decoded or copied, bounded by
    /// the change, not the map — and `None` if the caller must [`Doc2d::read`] again.
    ///
    /// Each key re-reads only what it belongs to, with the same readers [`Doc2d::read`]
    /// uses (so the rules live in one place):
    ///
    /// * a tile-map chunk of a layer the document has: that one chunk is decoded (the
    ///   layer is copied only if someone else still shares it);
    /// * a layer's own field (`name`, `visible`): that layer's fields, by exact lookup —
    ///   its chunks are kept, so renaming a layer of a 1,024-chunk map decodes nothing; a
    ///   layer that appears is read whole (its chunks only), one that is gone is dropped;
    /// * a map's own field (`name`, `tileset`): the map's fields, by exact lookup; a map
    ///   that appears is read whole, one that is gone is dropped;
    /// * anything under a tile set, a sheet or a rig: that one object is re-read (they are
    ///   small: rules, animations, bones);
    /// * a key outside the four `d2.` object kinds changes nothing (`read` ignores it).
    ///
    /// `None` only for a layer or chunk key not spelled canonically (`layer.01`,
    /// `chunk.c01_2`): `read` resolves two keys naming one layer or chunk by key order,
    /// so those re-read whole.
    pub fn apply_setting(&mut self, m: &ProjectMirror, key: &str) -> Option<usize> {
        let Some(rest) = key.strip_prefix("d2.") else {
            return Some(0);
        };
        let Some((kind, rest)) = rest.split_once('.') else {
            return Some(0);
        };
        // `objects` ignores a key naming an object with no field (`d2.rig.r`).
        let Some((id, field)) = rest.split_once('.') else {
            return Some(0);
        };
        match kind {
            "tilemap" => self.apply_tilemap(m, id, field),
            "tileset" => {
                self.reload_object(m, TILESET, id, |d, id, f, p| {
                    match f {
                        Some(f) => {
                            let t = read_tileset(id, f, p);
                            d.tilesets.insert(id.to_string(), t);
                        }
                        None => {
                            d.tilesets.remove(id);
                        }
                    };
                });
                Some(0)
            }
            "sheet" => {
                self.reload_object(m, SHEET, id, |d, id, f, p| {
                    match f {
                        Some(f) => {
                            let s = read_sheet(id, f, p);
                            d.sheets.insert(id.to_string(), s);
                        }
                        None => {
                            d.sheets.remove(id);
                        }
                    };
                });
                Some(0)
            }
            "rig" => {
                self.reload_object(m, RIG, id, |d, id, f, p| {
                    match f {
                        Some(f) => {
                            let r = read_rig(id, f, p);
                            d.rigs.insert(id.to_string(), r);
                        }
                        None => {
                            d.rigs.remove(id);
                        }
                    };
                });
                Some(0)
            }
            _ => Some(0),
        }
    }

    /// Re-read one small object (`kind.id.*`) with `put`, replacing its problems.
    fn reload_object(
        &mut self,
        m: &ProjectMirror,
        kind: &str,
        id: &str,
        put: impl FnOnce(&mut Doc2d, &str, Option<&BTreeMap<&str, &Value>>, &mut Vec<String>),
    ) {
        let at = format!("{kind}.{id}.");
        self.problems.retain(|p| !p.starts_with(&at));
        let f = fields_of(m, &at);
        let mut problems = Vec::new();
        put(self, id, (!f.is_empty()).then_some(&f), &mut problems);
        self.problems.extend(problems);
    }

    fn apply_tilemap(&mut self, m: &ProjectMirror, map: &str, field: &str) -> Option<usize> {
        let at = format!("{TILEMAP}.{map}");
        let map_prefix = format!("{at}.");
        // Canonical spelling first: a non-canonical layer or chunk key re-reads whole.
        let layer_field = field.strip_prefix("layer.").and_then(|r| r.split_once('.'));
        if let Some((li, lf)) = layer_field {
            let index = li.parse::<u32>().ok()?;
            if self.aliased_chunks || li != index.to_string() {
                return None;
            }
            if let Some(seg) = lf.strip_prefix("chunk.")
                && parse_chunk_segment(seg).map(chunk_segment).as_deref() != Some(seg)
            {
                return None;
            }
        }
        if !self.tilemaps.contains_key(map) {
            // A map that appeared (or a key of one that is still gone): read it whole —
            // its own keys only.
            let f = fields_of(m, &map_prefix);
            if f.is_empty() {
                return Some(0);
            }
            let mut aliased = false;
            let mut problems = Vec::new();
            let t = read_tilemap(map, &f, &mut problems, &mut aliased);
            if aliased {
                return None;
            }
            let work = t.layers.values().map(|l| l.chunks.len()).sum();
            self.problems.retain(|p| !p.starts_with(&map_prefix));
            self.problems.extend(problems);
            self.tilemaps.insert(map.to_string(), t);
            return Some(work);
        }
        if m.settings_under(&map_prefix).next().is_none() {
            // Every key of the map is gone.
            self.tilemaps.remove(map);
            self.problems.retain(|p| !p.starts_with(&map_prefix));
            return Some(0);
        }
        let Some((li, lf)) = layer_field else {
            // One of the map's own fields (or a field `read` ignores): re-read them by
            // exact lookup.
            let mut problems = Vec::new();
            let name = setting_text(m, &at, "name", map, &mut problems);
            let tileset = setting_text(m, &at, "tileset", "", &mut problems);
            for f in ["name", "tileset"] {
                let p = format!("{at}.{f}: ");
                self.problems.retain(|x| !x.starts_with(&p));
            }
            self.problems.extend(problems);
            let t = self.tilemaps.get_mut(map)?;
            t.name = name;
            t.tileset = tileset;
            return Some(0);
        };
        let index = li.parse::<u32>().ok()?;
        let lat = format!("{at}.layer.{index}");
        let layer_prefix = format!("{lat}.");
        if let Some(seg) = lf.strip_prefix("chunk.")
            && self
                .tilemaps
                .get(map)
                .is_some_and(|t| t.layers.contains_key(&index))
        {
            return Some(self.apply_chunk(m, map, index, &lat, seg));
        }
        if m.settings_under(&layer_prefix).next().is_none() {
            if let Some(t) = self.tilemaps.get_mut(map) {
                t.layers.remove(&index);
            }
            self.problems.retain(|p| !p.starts_with(&layer_prefix));
            return Some(0);
        }
        let have = self
            .tilemaps
            .get(map)
            .is_some_and(|t| t.layers.contains_key(&index));
        if !have {
            // A layer that appeared: read it whole (its own chunks only).
            let f = fields_of(m, &layer_prefix);
            let mut aliased = false;
            let mut problems = Vec::new();
            let layer = read_layer(index, li, &lat, &f, &mut problems, &mut aliased);
            if aliased {
                return None;
            }
            let work = layer.chunks.len();
            self.problems.retain(|p| !p.starts_with(&layer_prefix));
            self.problems.extend(problems);
            self.tilemaps
                .get_mut(map)?
                .layers
                .insert(index, Arc::new(layer));
            return Some(work);
        }
        // One of the layer's own fields: re-read them by exact lookup; the chunks stay.
        let mut problems = Vec::new();
        let name = setting_text(m, &lat, "name", &format!("Layer {index}"), &mut problems);
        let visible = match m.setting(&format!("{lat}.visible")) {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(v) => {
                problems.push(format!(
                    "{lat}.visible: expected true/false, found {}",
                    v.kind()
                ));
                true
            }
        };
        for f in ["name", "visible"] {
            let p = format!("{lat}.{f}: ");
            self.problems.retain(|x| !x.starts_with(&p));
        }
        self.problems.extend(problems);
        let layer = self.tilemaps.get_mut(map)?.layers.get_mut(&index)?;
        let mut copied = 0;
        if layer.name != name || layer.visible != visible {
            // Only the two fields change; a layer someone still shares is copied (counted).
            if Arc::strong_count(layer) > 1 {
                copied = layer.chunks.len();
            }
            let l = Arc::make_mut(layer);
            l.name = name;
            l.visible = visible;
        }
        Some(copied)
    }

    /// One chunk of a layer the document has (canonical spelling checked by the caller).
    fn apply_chunk(
        &mut self,
        m: &ProjectMirror,
        map: &str,
        index: u32,
        lat: &str,
        seg: &str,
    ) -> usize {
        let key = format!("{lat}.chunk.{seg}");
        let Some(layer) = self
            .tilemaps
            .get_mut(map)
            .and_then(|t| t.layers.get_mut(&index))
        else {
            return 0;
        };
        let copied = if Arc::strong_count(layer) > 1 {
            layer.chunks.len()
        } else {
            0
        };
        let layer = Arc::make_mut(layer);
        let prefix = format!("{key}: ");
        self.problems.retain(|p| !p.starts_with(&prefix));
        let Some(v) = m.setting(&key) else {
            // Cleared: the chunk is gone (a bad coordinate never had one).
            if let Some(c) = parse_chunk_segment(seg) {
                layer.chunks.remove(&c);
            }
            return copied;
        };
        match decode_chunk(seg, v) {
            Ok((c, ch)) => {
                layer.chunks.insert(c, ch);
            }
            Err(e) => {
                if let Some(c) = parse_chunk_segment(seg) {
                    layer.chunks.remove(&c);
                }
                self.problems.push(format!("{prefix}{e}"));
            }
        }
        copied + 1
    }

    /// The chunks across every layer of every map (what a full [`Doc2d::read`] decodes).
    pub fn chunk_count(&self) -> usize {
        self.tilemaps
            .values()
            .flat_map(|t| t.layers.values())
            .map(|l| l.chunks.len())
            .sum()
    }
}

// ---- the per-object readers (one set of rules for `read` and `apply_setting`) ----------

/// The fields of one object: every key under `prefix` (which ends in `.`), relative to it.
fn fields_of<'a>(m: &'a ProjectMirror, prefix: &str) -> BTreeMap<&'a str, &'a Value> {
    m.settings_under(prefix)
        .map(|(k, v)| (&k[prefix.len()..], v))
        .collect()
}

/// A text field read by exact lookup (see [`super::text`]).
fn setting_text(
    m: &ProjectMirror,
    at: &str,
    field: &str,
    default: &str,
    problems: &mut Vec<String>,
) -> String {
    match m.setting(&format!("{at}.{field}")) {
        None => default.to_string(),
        Some(Value::Text(t)) => t.clone(),
        Some(v) => {
            problems.push(format!("{at}.{field}: expected text, found {}", v.kind()));
            default.to_string()
        }
    }
}

fn read_tileset(id: &str, f: &BTreeMap<&str, &Value>, p: &mut Vec<String>) -> Tileset {
    let at = format!("{TILESET}.{id}");
    let mut terrains = BTreeMap::new();
    for (tid, tf) in sub_objects(f, "terrain") {
        let tat = format!("{at}.terrain.{tid}");
        let mut rules = BTreeMap::new();
        for (k, v) in &tf {
            let Some(mask) = k.strip_prefix("rule.") else {
                continue;
            };
            match (mask.parse::<u8>(), v) {
                (Ok(mk), Value::Int(t)) if *t >= 0 && canonical(mk) == mk => {
                    rules.insert(mk, u32::try_from(*t).unwrap_or(u32::MAX));
                }
                _ => p.push(format!("{tat}.{k}: not a canonical mask with a tile")),
            }
        }
        terrains.insert(
            tid.to_string(),
            Terrain {
                id: tid.to_string(),
                name: text(&tf, "name", tid, p, &tat),
                rules,
            },
        );
    }
    Tileset {
        id: id.to_string(),
        name: text(f, "name", id, p, &at),
        image: text(f, "image", "", p, &at),
        tile_w: int(f, "tile_w", 16, p, &at).max(1),
        tile_h: int(f, "tile_h", 16, p, &at).max(1),
        columns: int(f, "columns", 8, p, &at).max(1),
        count: int(f, "count", 64, p, &at).max(0),
        terrains,
    }
}

/// One layer from its fields (`name`, `visible`, `chunk.<c>`), `li` as spelled in its keys.
fn read_layer(
    index: u32,
    li: &str,
    lat: &str,
    lf: &BTreeMap<&str, &Value>,
    p: &mut Vec<String>,
    aliased: &mut bool,
) -> Layer {
    let mut chunks = BTreeMap::new();
    for (k, v) in lf {
        let Some(seg) = k.strip_prefix("chunk.") else {
            continue;
        };
        if li != index.to_string()
            || parse_chunk_segment(seg).map(chunk_segment).as_deref() != Some(seg)
        {
            *aliased = true;
        }
        match decode_chunk(seg, v) {
            Ok((c, ch)) => {
                chunks.insert(c, ch);
            }
            Err(e) => p.push(format!("{lat}.{k}: {e}")),
        }
    }
    Layer {
        index,
        name: text(lf, "name", &format!("Layer {index}"), p, lat),
        visible: super::boolean(lf, "visible", true, p, lat),
        chunks,
    }
}

fn read_tilemap(
    id: &str,
    f: &BTreeMap<&str, &Value>,
    p: &mut Vec<String>,
    aliased: &mut bool,
) -> Tilemap {
    let at = format!("{TILEMAP}.{id}");
    let mut layers = BTreeMap::new();
    for (li, lf) in sub_objects(f, "layer") {
        let lat = format!("{at}.layer.{li}");
        let Ok(index) = li.parse::<u32>() else {
            p.push(format!("{lat}: a layer index must be a number"));
            continue;
        };
        let layer = read_layer(index, li, &lat, &lf, p, aliased);
        layers.insert(index, Arc::new(layer));
    }
    Tilemap {
        id: id.to_string(),
        name: text(f, "name", id, p, &at),
        tileset: text(f, "tileset", "", p, &at),
        layers,
    }
}

fn read_sheet(id: &str, f: &BTreeMap<&str, &Value>, p: &mut Vec<String>) -> SpriteSheet {
    let at = format!("{SHEET}.{id}");
    let mut anims = BTreeMap::new();
    for (aid, af) in sub_objects(f, "anim") {
        let aat = format!("{at}.anim.{aid}");
        let frames = match parse_frames(&text(&af, "frames", "", p, &aat)) {
            Ok(f) => f,
            Err(e) => {
                p.push(format!("{aat}.frames: {e}"));
                Vec::new()
            }
        };
        anims.insert(
            aid.to_string(),
            Anim {
                id: aid.to_string(),
                name: text(&af, "name", aid, p, &aat),
                frames,
                fps: float(&af, "fps", 12.0, p, &aat),
                looping: super::boolean(&af, "looping", true, p, &aat),
            },
        );
    }
    let i = |k: &str, d: i64, p: &mut Vec<String>| int(f, k, d, p, &at);
    let slice = SheetSlice {
        image_w: i("image_w", 256, p).max(0),
        image_h: i("image_h", 256, p).max(0),
        cell_w: i("cell_w", 32, p).max(1),
        cell_h: i("cell_h", 32, p).max(1),
        offset_x: i("offset_x", 0, p).max(0),
        offset_y: i("offset_y", 0, p).max(0),
        spacing_x: i("spacing_x", 0, p).max(0),
        spacing_y: i("spacing_y", 0, p).max(0),
    };
    SpriteSheet {
        id: id.to_string(),
        name: text(f, "name", id, p, &at),
        image: text(f, "image", "", p, &at),
        slice,
        anims,
    }
}

fn read_rig(id: &str, f: &BTreeMap<&str, &Value>, p: &mut Vec<String>) -> Rig2d {
    let at = format!("{RIG}.{id}");
    let mut bones = BTreeMap::new();
    for (bid, bf) in sub_objects(f, "bone") {
        let bat = format!("{at}.bone.{bid}");
        let parent = text(&bf, "parent", "", p, &bat);
        bones.insert(
            bid.to_string(),
            Bone {
                id: bid.to_string(),
                name: text(&bf, "name", bid, p, &bat),
                parent: (!parent.is_empty()).then_some(parent),
                x: float(&bf, "x", 0.0, p, &bat),
                y: float(&bf, "y", 0.0, p, &bat),
                rot: float(&bf, "rot", 0.0, p, &bat),
                len: float(&bf, "len", 32.0, p, &bat),
                sprite: text(&bf, "sprite", "", p, &bat),
                z: int(&bf, "z", 0, p, &bat),
            },
        );
    }
    Rig2d {
        id: id.to_string(),
        name: text(f, "name", id, p, &at),
        bones,
    }
}

/// A chunk setting's coordinate (from the key segment after `chunk.`) and cells.
fn decode_chunk(seg: &str, v: &Value) -> Result<((i64, i64), Chunk), String> {
    let parsed = parse_chunk_segment(seg).ok_or("bad chunk coordinate".to_string());
    let chunk = match v {
        Value::Text(t) => Chunk::parse(t),
        other => Err(format!("expected text, found {}", other.kind())),
    };
    match (parsed, chunk) {
        (Ok(c), Ok(ch)) => Ok((c, ch)),
        (Err(e), _) | (_, Err(e)) => Err(e),
    }
}

// ---- the backend ------------------------------------------------------------------------

/// What the 2D editors ask of the 2D pipeline (Ch.35). [`Forge2dScene`] answers it with
/// `forge-2d` (M4-11); a host may put another implementation behind it.
pub trait Scene2d {
    fn backend(&self) -> BackendInfo;
    /// The tile a cell of `terrain` shows given its raw 8-neighbour mask (`None`: no rule
    /// fits, the cell shows as unresolved).
    fn autotile(&self, terrain: &Terrain, mask: u8) -> Option<u32>;
    /// The frames a slicing yields, row-major (cells that would cross the image edge are
    /// not frames).
    fn slice(&self, s: &SheetSlice) -> Vec<FrameRect>;
    /// Forward kinematics of a rig.
    fn pose(&self, rig: &Rig2d) -> Pose;
    /// The pixel size of a source image, if the backend can read it.
    fn image_size(&self, image: &str) -> Option<(i64, i64)>;
}

/// Reads a project file's bytes by project path (`None`: no such file).
pub type SourceReader = Rc<dyn Fn(&str) -> Option<Vec<u8>>>;

/// The 2D editors over the real 2D pipeline, `forge-2d` (Ch.35, M4-11): tiles are solved by
/// `forge_2d::tilemap::autotile` (the rule the game's tile maps draw with), sheets are cut by
/// `forge_2d::anim::slice`, rigs are posed by `forge_2d::skeleton::Skeleton2d` (the cutout
/// runtime's forward kinematics, deterministic trigonometry), and image sizes are read from
/// the file headers (`forge_2d::atlas::image_file_size`: PNG, Aseprite) through the host's
/// [`SourceReader`], or from sizes the host registers.
#[derive(Default)]
pub struct Forge2dScene {
    source: Option<SourceReader>,
    images: std::cell::RefCell<BTreeMap<String, (i64, i64)>>,
}

impl std::fmt::Debug for Forge2dScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forge2dScene")
            .field("source", &self.source.is_some())
            .field("images", &self.images.borrow().len())
            .finish()
    }
}

impl Forge2dScene {
    /// No project source: image sizes only as registered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reading source images through `source` (the project's files).
    pub fn with_source(source: SourceReader) -> Self {
        Self {
            source: Some(source),
            images: std::cell::RefCell::default(),
        }
    }

    /// Tell the backend an image's size (an import the asset database already measured).
    pub fn register_image(&self, image: &str, w: i64, h: i64) {
        self.images.borrow_mut().insert(image.to_string(), (w, h));
    }
}

fn to_u32(v: i64) -> u32 {
    u32::try_from(v.max(0)).unwrap_or(u32::MAX)
}

impl Scene2d for Forge2dScene {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "forge-2d".into(),
            in_memory: false,
            note: forge_ui::tr_key!(
                "Tiles, sheets and rigs are solved by the 2D pipeline (forge-2d), the same code the game runs."
            )
            .into(),
        }
    }

    fn autotile(&self, terrain: &Terrain, mask: u8) -> Option<u32> {
        forge_2d::tilemap::autotile(&terrain.rules, mask)
    }

    fn slice(&self, s: &SheetSlice) -> Vec<FrameRect> {
        let cut = forge_2d::anim::SheetSlice {
            image_w: to_u32(s.image_w),
            image_h: to_u32(s.image_h),
            cell_w: to_u32(s.cell_w),
            cell_h: to_u32(s.cell_h),
            offset_x: to_u32(s.offset_x),
            offset_y: to_u32(s.offset_y),
            spacing_x: to_u32(s.spacing_x),
            spacing_y: to_u32(s.spacing_y),
        };
        forge_2d::anim::slice(&cut)
            .into_iter()
            .map(|f| FrameRect {
                index: f.index,
                x: i64::from(f.x),
                y: i64::from(f.y),
                w: i64::from(f.w),
                h: i64::from(f.h),
            })
            .collect()
    }

    fn pose(&self, rig: &Rig2d) -> Pose {
        use forge_2d::DVec2;
        use forge_2d::skeleton::{BoneLocal, Skeleton2d};
        let bones: Vec<(String, Option<String>, BoneLocal, f64)> = rig
            .bones
            .values()
            .map(|b| {
                (
                    b.id.clone(),
                    b.parent.clone(),
                    BoneLocal {
                        offset: DVec2::new(b.x, b.y),
                        angle: b.rot.to_radians(),
                        scale: DVec2::new(1.0, 1.0),
                    },
                    b.len,
                )
            })
            .collect();
        let (sk, _, broken) = Skeleton2d::from_unordered(&bones);
        let world = sk.world(&sk.rest_pose(), &forge_2d::math::Xform2::IDENTITY);
        let mut posed: Vec<BonePose> = sk
            .bones
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                let x = world.get(i)?;
                let tip = sk.tip(&world, i)?;
                Some(BonePose {
                    id: b.name.clone(),
                    origin: (x.translation.x, x.translation.y),
                    tip: (tip.x, tip.y),
                    angle: x.rot.angle().to_degrees(),
                })
            })
            .collect();
        posed.sort_by(|a, b| a.id.cmp(&b.id));
        Pose {
            bones: posed,
            broken,
        }
    }

    fn image_size(&self, image: &str) -> Option<(i64, i64)> {
        if let Some(s) = self.images.borrow().get(image) {
            return Some(*s);
        }
        let bytes = self.source.as_ref().and_then(|read| read(image))?;
        let (w, h) = forge_2d::atlas::image_file_size(&bytes)?;
        let size = (i64::from(w), i64::from(h));
        self.images.borrow_mut().insert(image.to_string(), size);
        Some(size)
    }
}

// ---- edits --------------------------------------------------------------------------------

/// Field `field` of object `id` under `root`.
pub fn key(root: &str, id: &str, field: &str) -> String {
    format!("{root}.{id}.{field}")
}

/// A new tile set with `count` tiles in `columns` columns and a first terrain.
pub fn new_tileset(doc: &Doc2d, name: &str) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| doc.tilesets.contains_key(s));
    let cmds = vec![
        set(key(TILESET, &id, "name"), Value::Text(name.into())),
        set(key(TILESET, &id, "tile_w"), Value::Int(16)),
        set(key(TILESET, &id, "tile_h"), Value::Int(16)),
        set(key(TILESET, &id, "columns"), Value::Int(8)),
        set(key(TILESET, &id, "count"), Value::Int(48)),
        set(
            key(TILESET, &id, "terrain.ground.name"),
            Value::Text("Ground".into()),
        ),
    ];
    (id, cmds)
}

/// Fill a terrain's rules from the blob layout: the `i`-th canonical mask shows tile
/// `first + i` (the 47-tile template many tile sets are drawn in).
pub fn blob_template(tileset: &str, terrain: &str, first: u32) -> Vec<EditorCommand> {
    blob47()
        .into_iter()
        .enumerate()
        .map(|(i, m)| {
            set(
                key(TILESET, tileset, &format!("terrain.{terrain}.rule.{m}")),
                Value::Int(i64::from(first) + i as i64),
            )
        })
        .collect()
}

/// A new tile map over `tileset` with one layer.
pub fn new_tilemap(doc: &Doc2d, name: &str, tileset: &str) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| doc.tilemaps.contains_key(s));
    let cmds = vec![
        set(key(TILEMAP, &id, "name"), Value::Text(name.into())),
        set(key(TILEMAP, &id, "tileset"), Value::Text(tileset.into())),
        set(
            key(TILEMAP, &id, "layer.0.name"),
            Value::Text("Ground".into()),
        ),
        set(key(TILEMAP, &id, "layer.0.visible"), Value::Bool(true)),
    ];
    (id, cmds)
}

/// A new sprite sheet (sliced into 32 px cells until edited).
pub fn new_sheet(
    doc: &Doc2d,
    name: &str,
    image: &str,
    size: (i64, i64),
) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| doc.sheets.contains_key(s));
    let cmds = vec![
        set(key(SHEET, &id, "name"), Value::Text(name.into())),
        set(key(SHEET, &id, "image"), Value::Text(image.into())),
        set(key(SHEET, &id, "image_w"), Value::Int(size.0)),
        set(key(SHEET, &id, "image_h"), Value::Int(size.1)),
        set(key(SHEET, &id, "cell_w"), Value::Int(32)),
        set(key(SHEET, &id, "cell_h"), Value::Int(32)),
    ];
    (id, cmds)
}

/// A new rig with a root bone.
pub fn new_rig(doc: &Doc2d, name: &str) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| doc.rigs.contains_key(s));
    let cmds = vec![
        set(key(RIG, &id, "name"), Value::Text(name.into())),
        set(key(RIG, &id, "bone.root.name"), Value::Text("Root".into())),
        set(key(RIG, &id, "bone.root.len"), Value::Float(48.0)),
    ];
    (id, cmds)
}

/// A new bone under `parent`, at the parent's tip.
pub fn new_bone(rig: &Rig2d, name: &str, parent: Option<&str>) -> (String, Vec<EditorCommand>) {
    let id = super::unique_id(&super::ident_from(name), |s| rig.bones.contains_key(s));
    let k = |f: &str| key(RIG, &rig.id, &format!("bone.{id}.{f}"));
    let x = parent.and_then(|p| rig.bones.get(p)).map_or(0.0, |p| p.len);
    let mut cmds = vec![
        set(k("name"), Value::Text(name.into())),
        set(k("x"), Value::Float(x)),
        set(k("len"), Value::Float(32.0)),
    ];
    if let Some(p) = parent {
        cmds.push(set(k("parent"), Value::Text(p.into())));
    }
    (id, cmds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_reduction_has_47_shapes_and_corners_need_both_edges() {
        assert_eq!(blob47().len(), 47);
        assert_eq!(canonical(NE), 0, "a lone corner cannot show");
        assert_eq!(canonical(N | E | NE), N | E | NE);
        assert_eq!(canonical(N | NE), N);
        assert_eq!(canonical(0xFF), 0xFF);
    }

    #[test]
    fn chunks_round_trip_and_coordinates_cover_negative_cells() {
        let mut ch = Chunk::default();
        ch.cells[0] = Cell::Tile(7);
        ch.cells[255] = Cell::Terrain("grass".into());
        let back = Chunk::parse(&ch.encode()).unwrap_or_default();
        assert_eq!(back, ch);
        assert!(Chunk::parse("t1,t2").is_err());
        assert_eq!(chunk_of(-1, -1), ((-1, -1), 255));
        assert_eq!(chunk_of(16, 0), ((1, 0), 0));
        for c in [(0, 0), (-3, 12), (7, -1)] {
            let s = chunk_segment(c);
            assert!(super::super::is_ident(&s), "{s}");
            assert_eq!(parse_chunk_segment(&s), Some(c));
        }
    }

    #[test]
    fn slicing_respects_offset_spacing_and_edges() {
        let b = Forge2dScene::new();
        let s = SheetSlice {
            image_w: 100,
            image_h: 50,
            cell_w: 30,
            cell_h: 20,
            offset_x: 2,
            offset_y: 3,
            spacing_x: 2,
            spacing_y: 4,
        };
        let f = b.slice(&s);
        // x: 2, 34, 66 (66+30=96 ≤ 100); y: 3, 27 (27+20=47 ≤ 50).
        assert_eq!(f.len(), 6);
        assert_eq!((f[4].x, f[4].y, f[4].index), (34, 27, 4));
    }

    #[test]
    fn forward_kinematics_and_loops() {
        let b = Forge2dScene::new();
        let mut rig = Rig2d::default();
        rig.bones.insert(
            "a".into(),
            Bone {
                id: "a".into(),
                len: 10.0,
                rot: 90.0,
                ..Bone::default()
            },
        );
        rig.bones.insert(
            "b".into(),
            Bone {
                id: "b".into(),
                parent: Some("a".into()),
                x: 10.0,
                len: 5.0,
                ..Bone::default()
            },
        );
        let p = b.pose(&rig);
        let bb = p
            .bones
            .iter()
            .find(|x| x.id == "b")
            .cloned()
            .unwrap_or(BonePose {
                id: String::new(),
                origin: (0.0, 0.0),
                tip: (0.0, 0.0),
                angle: 0.0,
            });
        assert!((bb.origin.0).abs() < 1e-9 && (bb.origin.1 - 10.0).abs() < 1e-9);
        assert!((bb.tip.1 - 15.0).abs() < 1e-9);
        assert!(rig.would_cycle("a", Some("b")));
        assert!(!rig.would_cycle("b", None));
        if let Some(a) = rig.bones.get_mut("a") {
            a.parent = Some("b".into());
        }
        let p = b.pose(&rig);
        assert_eq!(
            p.broken.len(),
            2,
            "a loop poses nothing and names both bones"
        );
    }

    /// Drive a mirror through the bus (the only way it changes) and keep an incrementally
    /// updated `Doc2d` beside it: after every batch it must equal a full read.
    #[test]
    fn applying_changed_chunk_keys_equals_a_full_read() {
        use crate::client::BusClient;
        use crate::core::EditorCore;
        use forge_cmd::Issuer;

        fn pump(m: &mut ProjectMirror, c: &mut crate::core::LocalBus) {
            let p = c.pump();
            if p.gap.is_some() {
                let (project, next) = c.snapshot();
                m.resync(&project, next, c.history());
            }
            for ev in &p.events {
                m.apply(ev, |t| c.txn_info(t));
            }
        }
        fn same(a: &Doc2d, b: &Doc2d) -> bool {
            let (mut pa, mut pb) = (a.problems.clone(), b.problems.clone());
            pa.sort();
            pb.sort();
            a.tilemaps == b.tilemaps && a.tilesets == b.tilesets && pa == pb
        }
        let core = EditorCore::new();
        let mut c = EditorCore::connect(&core, Issuer::Test);
        let mut m = ProjectMirror::new();
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        let mut tile = Chunk::default();
        tile.cells[3] = Cell::Tile(9);
        let mut terr = Chunk::default();
        terr.cells[17] = Cell::Terrain("grass".into());
        let k = |cc: (i64, i64)| chunk_key("m", 0, cc);
        let text = |ch: &Chunk| Some(Value::Text(ch.encode()));
        let batches: Vec<Vec<(String, Option<Value>)>> = vec![
            vec![
                ("d2.tilemap.m.name".into(), Some(Value::Text("M".into()))),
                (
                    "d2.tilemap.m.layer.0.name".into(),
                    Some(Value::Text("G".into())),
                ),
                (k((0, 0)), text(&tile)),
            ],
            vec![(k((1, -2)), text(&terr)), (k((0, 0)), text(&terr))],
            vec![(k((5, 5)), Some(Value::Text("t1,t2".into())))],
            vec![(k((5, 5)), text(&tile)), (k((0, 0)), None)],
            vec![(k((1, -2)), Some(Value::Int(3)))],
            vec![(
                "d2.tilemap.m.layer.0.chunk.nonsense".into(),
                Some(Value::Text("x".into())),
            )],
            vec![("d2.tilemap.m.layer.0.chunk.nonsense".into(), None)],
            // An alias of chunk (5, 5): from here on edits re-read whole.
            vec![("d2.tilemap.m.layer.0.chunk.c05_5".into(), text(&terr))],
            vec![(k((5, 5)), None)],
        ];
        let mut doc = Doc2d::read(&m);
        let mut applied_in_place = 0;
        for b in batches {
            let seq = m.setting_change_seq();
            for (key, v) in b {
                c.apply(EditorCommand::SetSetting { key, value: v }, None);
            }
            pump(&mut m, &mut c);
            let keys: Vec<String> = m
                .setting_changes_since(seq)
                .map(|i| i.map(str::to_string).collect())
                .unwrap_or_default();
            let mut full = false;
            for key in &keys {
                match doc.apply_setting(&m, key) {
                    Some(w) => {
                        applied_in_place += 1;
                        assert!(w <= 1, "{key}: one chunk changed, {w} decoded or copied");
                    }
                    None => full = true,
                }
            }
            if full {
                doc = Doc2d::read(&m);
            }
            let want = Doc2d::read(&m);
            assert!(same(&doc, &want), "after {keys:?}:\n{doc:?}\n!=\n{want:?}");
        }
        assert!(applied_in_place >= 6, "{applied_in_place} applied in place");
        assert!(doc.aliased_chunks);
        // A layer someone else still holds is copied, and says so.
        let mut doc = Doc2d::read(&m);
        doc.aliased_chunks = false;
        let held = doc
            .tilemaps
            .get("m")
            .and_then(|t| t.layers.get(&0))
            .cloned();
        let n = held.as_ref().map_or(0, |l| l.chunks.len());
        c.apply(
            EditorCommand::SetSetting {
                key: k((7, 7)),
                value: text(&tile),
            },
            None,
        );
        pump(&mut m, &mut c);
        assert_eq!(doc.apply_setting(&m, &k((7, 7))), Some(n + 1));
        assert!(held.is_some_and(|l| !l.chunks.contains_key(&(7, 7))));
    }

    #[test]
    fn object_keys_apply_in_place_and_equal_a_full_read() {
        use crate::client::BusClient;
        use crate::core::EditorCore;
        use forge_cmd::Issuer;

        let core = EditorCore::new();
        let mut c = EditorCore::connect(&core, Issuer::Test);
        let mut m = ProjectMirror::new();
        let (project, next) = c.snapshot();
        m.resync(&project, next, c.history());
        let pump = |m: &mut ProjectMirror, c: &mut crate::core::LocalBus| {
            let p = c.pump();
            for ev in &p.events {
                m.apply(ev, |t| c.txn_info(t));
            }
        };
        let mut tile = Chunk::default();
        tile.cells[3] = Cell::Tile(9);
        let t = |s: &str| Some(Value::Text(s.into()));
        let i = |n: i64| Some(Value::Int(n));
        // A map with 40 chunks: renaming its layer must decode none of them.
        let mut big: Vec<(String, Option<Value>)> = (0..40)
            .map(|x| {
                (
                    chunk_key("big", 0, (x, 0)),
                    Some(Value::Text(tile.encode())),
                )
            })
            .collect();
        big.push(("d2.tilemap.big.name".into(), t("Big")));
        let batches: Vec<Vec<(String, Option<Value>)>> = vec![
            big,
            vec![("d2.tilemap.big.layer.0.name".into(), t("Ground"))],
            vec![("d2.tilemap.big.layer.0.visible".into(), i(1))],
            vec![(
                "d2.tilemap.big.layer.0.visible".into(),
                Some(Value::Bool(false)),
            )],
            vec![("d2.tilemap.big.tileset".into(), i(4))],
            vec![("d2.tilemap.big.tileset".into(), t("ts"))],
            vec![
                ("d2.tileset.ts.name".into(), t("Tiles")),
                ("d2.tileset.ts.terrain.grass.name".into(), t("Grass")),
                ("d2.tileset.ts.terrain.grass.rule.0".into(), i(5)),
                ("d2.tileset.ts.terrain.grass.rule.2".into(), i(6)),
            ],
            vec![("d2.tileset.ts.terrain.grass.rule.2".into(), None)],
            vec![("d2.tileset.ts.tile_w".into(), t("wide"))],
            vec![("d2.tileset.ts.tile_w".into(), i(32))],
            vec![
                ("d2.sheet.hero.name".into(), t("Hero")),
                ("d2.sheet.hero.anim.run.frames".into(), t("0,1,x")),
            ],
            vec![("d2.sheet.hero.anim.run.frames".into(), t("0,1,2"))],
            vec![
                ("d2.rig.r.name".into(), t("Rig")),
                ("d2.rig.r.bone.hip.len".into(), Some(Value::Float(10.0))),
                ("d2.rig.r.bone.arm.parent".into(), t("hip")),
            ],
            vec![("d2.rig.r.bone.arm.parent".into(), None)],
            // A new layer announced by its name, then its chunk.
            vec![("d2.tilemap.big.layer.1.name".into(), t("Top"))],
            vec![(
                chunk_key("big", 1, (2, 2)),
                Some(Value::Text(tile.encode())),
            )],
            // Remove the whole layer 1, the rig and the sheet.
            vec![
                ("d2.tilemap.big.layer.1.name".into(), None),
                (chunk_key("big", 1, (2, 2)), None),
            ],
            vec![
                ("d2.rig.r.name".into(), None),
                ("d2.rig.r.bone.hip.len".into(), None),
            ],
            vec![
                ("d2.sheet.hero.name".into(), None),
                ("d2.sheet.hero.anim.run.frames".into(), None),
            ],
            // Keys `read` ignores.
            vec![("d2.rig.lonely".into(), i(1)), ("d2.other.x".into(), i(1))],
            // A map that appears with its chunk before its name.
            vec![(
                chunk_key("small", 0, (0, 0)),
                Some(Value::Text(tile.encode())),
            )],
        ];
        let mut doc = Doc2d::read(&m);
        for (n, b) in batches.into_iter().enumerate() {
            let seq = m.setting_change_seq();
            for (key, v) in b {
                c.apply(EditorCommand::SetSetting { key, value: v }, None);
            }
            pump(&mut m, &mut c);
            let keys: Vec<String> = m
                .setting_changes_since(seq)
                .map(|i| i.map(str::to_string).collect())
                .unwrap_or_default();
            let mut work = 0;
            for key in &keys {
                match doc.apply_setting(&m, key) {
                    Some(w) => work += w,
                    None => panic!("batch {n}: {key} was not applied in place"),
                }
            }
            if (1..=5).contains(&n) {
                assert_eq!(
                    work, 0,
                    "batch {n}: a field of a 40-chunk map decoded {work}"
                );
            }
            let want = Doc2d::read(&m);
            let (mut pa, mut pb) = (doc.problems.clone(), want.problems.clone());
            pa.sort();
            pb.sort();
            assert_eq!(pa, pb, "batch {n}: problems");
            assert_eq!(doc.tilemaps, want.tilemaps, "batch {n}: tile maps");
            assert_eq!(doc.tilesets, want.tilesets, "batch {n}: tile sets");
            assert_eq!(doc.sheets, want.sheets, "batch {n}: sheets");
            assert_eq!(doc.rigs, want.rigs, "batch {n}: rigs");
        }
        assert!(doc.rigs.is_empty() && doc.sheets.is_empty());
        assert_eq!(doc.tilemaps.len(), 2);
    }

    #[test]
    fn autotile_prefers_the_exact_shape_then_edges_then_isolated() {
        let b = Forge2dScene::new();
        let mut t = Terrain::default();
        t.rules.insert(0, 1);
        t.rules.insert(N | S, 2);
        t.rules.insert(N | E | NE, 3);
        assert_eq!(
            b.autotile(&t, N | S | NE),
            Some(2),
            "a lone corner is dropped"
        );
        assert_eq!(b.autotile(&t, N | E | NE), Some(3));
        assert_eq!(b.autotile(&t, N | E), Some(1), "no N|E rule: isolated");
    }
}
