//! The **2D editors** (`forge.editors_2d`, Ch.21 §21.21, DoD M2-66; Ch.35 §35.2): three
//! tabs over the 2D model in `forge_editor::domain::scene2d`.
//!
//! * **Tile palette.** Tile sets with autotile terrains, and layered tile maps. Pick a tile
//!   or a terrain as the brush and paint the map on the canvas: a drag is **one gesture,
//!   one undo entry**, and writes only the chunks it touched; a terrain cell's tile is
//!   solved from its neighbours by the 2D backend (the blob-47 rules). The rules list edits
//!   a terrain's rules: "Assign to rule" makes a palette click set the selected mask's tile,
//!   and "Blob template" fills all 47 from the standard layout.
//! * **Sprite sheet.** Slice a sheet by cell size, offset and spacing (the preview shows the
//!   frames), and build frame animations by picking frames.
//! * **2D rig.** A `Skeleton2D` cutout rig: a bone tree (rename in place, drag to reparent —
//!   a loop is refused with the reason — Delete removes a bone and its children), exact
//!   fields, and the pose view, where dragging a bone's tip rotates it as one gesture.
//!
//! Every project edit is a command (I7). Selections, the brush and the palette mode are
//! session state. Available under every preset (I15); the 2D preset opens it by default.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use forge_cmd::{EditorCommand, Value};
use forge_editor::domain::scene2d::{
    self as s2, Cell as MapCell, Doc2d, RIG, SHEET, Stroke, TILEMAP, TILESET, blob47,
};
use forge_editor::domain::{clear, clear_under, set};
use forge_editor::emitter::{CommandEmitter, Gesture};
use forge_editor::mirror::ProjectMirror;
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_editor::session::SessionState;
use forge_ui::widgets::{
    Button, Checkbox, Container, IntegerCommitted, IntegerField, Label, LabelKind,
    NumericCommitted, NumericField, Pressed, RadioGroup, RowItem, RowRenamed, RowsDeleteRequested,
    RowsDropped, SelectionChanged, SignalChanged, SignalRelay, Submitted, Tabs, TextField,
    VirtualTree,
};
use forge_ui::{Dirty, Key, NodeStyle, Role, Signal, Ui, WidgetId};

use crate::common::{Row, Rows, key_of, refuse, select, selected, show_rows, watch};
use crate::widgets::{
    BoneDrag, CanvasView, FramePicked, PaintCells, PaletteView, RigView, RigViewData, SheetPreview,
    SheetView, StrokePhase, TileCanvas, TilePalette, TilePicked,
};

/// The tab titles, in order.
pub const TABS: [&str; 3] = [
    forge_ui::tr_key!("Tile palette"),
    forge_ui::tr_key!("Sprite sheet"),
    forge_ui::tr_key!("2D rig"),
];
const PREFIX: &str = "d2.";

#[derive(Clone, Debug, PartialEq)]
enum TileRow {
    Tileset(String),
    Terrain(String, String),
}

#[derive(Clone, Debug, PartialEq)]
enum MapRow {
    Map(String),
    Layer(String, u32),
}

/// The brush.
#[derive(Clone, Debug, PartialEq)]
pub enum Brush {
    Tile(u32),
    Terrain(String),
}

/// The slicing fields, `(key, label)` in display order.
const SLICE_FIELDS: [(&str, &str); 8] = [
    ("image_w", forge_ui::tr_key!("Image width")),
    ("image_h", forge_ui::tr_key!("Image height")),
    ("cell_w", forge_ui::tr_key!("Cell width")),
    ("cell_h", forge_ui::tr_key!("Cell height")),
    ("offset_x", forge_ui::tr_key!("Offset X")),
    ("offset_y", forge_ui::tr_key!("Offset Y")),
    ("spacing_x", forge_ui::tr_key!("Spacing X")),
    ("spacing_y", forge_ui::tr_key!("Spacing Y")),
];

/// The bone number fields, `(key, label, unit)`.
const BONE_FIELDS: [(&str, &str, &str); 4] = [
    ("x", forge_ui::tr_key!("Offset X"), "px"),
    ("y", forge_ui::tr_key!("Offset Y"), "px"),
    ("rot", forge_ui::tr_key!("Rotation"), "\u{b0}"),
    ("len", forge_ui::tr_key!("Length"), "px"),
];

struct Ed {
    /// The panel's empty state: no tile set, map, sheet or rig yet (M2-70).
    nothing: WidgetId,
    doc: Doc2d,
    seen: u64,
    status: Signal<String>,
    problems: Signal<String>,
    // tile palette
    tiles: WidgetId,
    tile_rows: Rows<TileRow>,
    maps: WidgetId,
    map_rows: Rows<MapRow>,
    rules: WidgetId,
    rule_rows: Rows<u8>,
    canvas: WidgetId,
    canvas_view: Rc<RefCell<CanvasView>>,
    palette: WidgetId,
    palette_view: Rc<RefCell<PaletteView>>,
    palette_mode: Signal<usize>,
    ts_fields: Vec<(WidgetId, Signal<i128>, &'static str)>,
    brush: Option<Brush>,
    sel_tileset: Option<String>,
    sel_terrain: Option<String>,
    sel_layer: Option<(String, u32)>,
    sel_rule: Option<u8>,
    stroke: Option<StrokeState>,
    fault_no_gesture: bool,
    /// The setting-change sequence the model is up to date with (`None`: read it whole).
    setting_seq: Option<u64>,
    fault_full_reread: bool,
    fault_resend_stroke: bool,
    probe: Option<Rc<std::cell::Cell<usize>>>,
    // sprite sheet
    sheets: WidgetId,
    sheet_rows: Rows<String>,
    anims: WidgetId,
    anim_rows: Rows<String>,
    preview: WidgetId,
    sheet_view: Rc<RefCell<SheetView>>,
    slice_fields: Vec<(WidgetId, Signal<i128>, &'static str)>,
    fps: Signal<f64>,
    looping: Signal<bool>,
    sel_sheet: Option<String>,
    sel_anim: Option<String>,
    // rig
    rigs: WidgetId,
    rig_rows: Rows<String>,
    bones: WidgetId,
    bone_rows: Rows<String>,
    rig_view: WidgetId,
    rig_data: Rc<RefCell<RigViewData>>,
    bone_fields: Vec<(WidgetId, Signal<f64>, &'static str)>,
    bone_z: Signal<i128>,
    bone_sprite: Signal<String>,
    sel_rig: Option<String>,
    sel_bone: Option<String>,
    bone_drag: Option<Gesture>,
}

struct StrokeState {
    map: String,
    layer: u32,
    gesture: Option<Gesture>,
}

impl Ed {
    fn say(&self, ui: &mut Ui, s: impl Into<String>) {
        self.status.set(ui.rt_mut(), s.into());
    }

    /// Bring the model up to date (the `d2.` namespace changed). Every changed key is
    /// applied in place from the mirror's setting log (`Doc2d::apply_setting`): a paint
    /// stroke's step costs the chunks it wrote, not the map (D-5), and a rename, a rule, a
    /// sheet or a bone re-reads only its own object — a layer rename on a 1,024-chunk map
    /// decodes no chunk. Only a reader that fell behind the log, or a key spelled
    /// non-canonically, re-reads the namespace whole.
    fn refresh(&mut self, ui: &mut Ui, m: &ProjectMirror, sv: &EditorServices) {
        // The canvas shares the selected layer: let go of it first, so a chunk applied in
        // place copies nothing.
        self.canvas_view.borrow_mut().layer = None;
        let mut work = 0usize;
        let mut structural = false;
        let mut in_place = !self.fault_full_reread;
        if in_place {
            match self.setting_seq.and_then(|s| m.setting_changes_since(s)) {
                None => in_place = false,
                Some(keys) => {
                    // A key changed several times since the last refresh is applied once.
                    let mut done: HashSet<&str> = HashSet::new();
                    for k in keys {
                        if !k.starts_with(PREFIX) || !done.insert(k) {
                            continue;
                        }
                        structural |= !Doc2d::is_chunk_key(k);
                        match self.doc.apply_setting(m, k) {
                            Some(w) => work += w,
                            None => {
                                in_place = false;
                                break;
                            }
                        }
                    }
                }
            }
        }
        self.setting_seq = Some(m.setting_change_seq());
        if in_place && structural {
            self.model_changed(ui, sv);
        } else if in_place {
            self.show_problems(ui);
            self.show_canvas(ui);
        } else {
            self.reread(ui, m, sv);
            work += self.doc.chunk_count();
        }
        if let Some(p) = &self.probe {
            p.set(p.get() + work);
        }
    }

    fn show_problems(&mut self, ui: &mut Ui) {
        let p = if self.doc.problems.is_empty() {
            String::new()
        } else {
            forge_ui::trf!(
                "\u{26a0} {n} value(s) could not be read and were skipped: {first}",
                n = self.doc.problems.len(),
                first = self.doc.problems.first().cloned().unwrap_or_default()
            )
        };
        if self.problems.get(ui.rt()) != p {
            self.problems.set(ui.rt_mut(), p);
        }
    }

    /// Re-read the model whole.
    fn reread(&mut self, ui: &mut Ui, m: &ProjectMirror, sv: &EditorServices) {
        self.doc = Doc2d::read(m);
        self.model_changed(ui, sv);
    }

    /// The model changed beyond chunks: drop selections whose object is gone and show the
    /// lists and views again.
    fn model_changed(&mut self, ui: &mut Ui, sv: &EditorServices) {
        self.show_problems(ui);
        // A selection whose object is gone is dropped.
        if self
            .sel_tileset
            .as_ref()
            .is_some_and(|t| !self.doc.tilesets.contains_key(t))
        {
            self.sel_tileset = None;
            self.sel_terrain = None;
        }
        if let Some((mp, l)) = &self.sel_layer
            && !self
                .doc
                .tilemaps
                .get(mp)
                .is_some_and(|t| t.layers.contains_key(l))
        {
            self.sel_layer = None;
        }
        if self
            .sel_sheet
            .as_ref()
            .is_some_and(|s| !self.doc.sheets.contains_key(s))
        {
            self.sel_sheet = None;
            self.sel_anim = None;
        }
        if self
            .sel_rig
            .as_ref()
            .is_some_and(|r| !self.doc.rigs.contains_key(r))
        {
            self.sel_rig = None;
            self.sel_bone = None;
        }
        self.show_lists(ui);
        self.show_views(ui, sv);
    }

    fn show_lists(&mut self, ui: &mut Ui) {
        let none = self.doc.tilesets.is_empty()
            && self.doc.tilemaps.is_empty()
            && self.doc.sheets.is_empty()
            && self.doc.rigs.is_empty();
        let _ = ui.set_hidden(self.nothing, !none);
        // Tile sets and their terrains.
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        for ts in self.doc.tilesets.values() {
            let k = key_of(&["ts", &ts.id]);
            rows.push((
                None,
                k,
                RowItem::new(forge_ui::trf!(
                    "{name} ({count} tiles)",
                    name = ts.name,
                    count = ts.count
                )),
            ));
            map.insert(k, TileRow::Tileset(ts.id.clone()));
            for tr in ts.terrains.values() {
                let kk = key_of(&["ts", &ts.id, &tr.id]);
                rows.push((
                    Some(k),
                    kk,
                    RowItem::new(forge_ui::trf!(
                        "{name} \u{2014} {rules_count} of 47 rules",
                        name = tr.name,
                        rules_count = tr.rules.len()
                    )),
                ));
                map.insert(kk, TileRow::Terrain(ts.id.clone(), tr.id.clone()));
            }
        }
        show_rows(ui, self.tiles, rows, map, &mut self.tile_rows);
        // Tile maps and their layers.
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        for tm in self.doc.tilemaps.values() {
            let k = key_of(&["tm", &tm.id]);
            let ts = self
                .doc
                .tilesets
                .get(&tm.tileset)
                .map_or(forge_ui::tr!("no tile set"), |t| t.name.as_str());
            rows.push((None, k, RowItem::new(format!("{} ({ts})", tm.name))));
            map.insert(k, MapRow::Map(tm.id.clone()));
            for l in tm.layers.values() {
                let kk = key_of(&["tm", &tm.id, &l.index.to_string()]);
                rows.push((
                    Some(k),
                    kk,
                    RowItem::new(forge_ui::trf!(
                        "Layer {index}: {name}",
                        index = l.index,
                        name = l.name
                    ))
                    .muted(!l.visible),
                ));
                map.insert(kk, MapRow::Layer(tm.id.clone(), l.index));
            }
        }
        show_rows(ui, self.maps, rows, map, &mut self.map_rows);
        self.show_rules(ui);
        // Sheets and the selected sheet's animations.
        let rows: Vec<Row> = self
            .doc
            .sheets
            .values()
            .map(|s| (None, key_of(&["sh", &s.id]), RowItem::new(s.name.clone())))
            .collect();
        let map = self
            .doc
            .sheets
            .keys()
            .map(|id| (key_of(&["sh", id]), id.clone()))
            .collect();
        show_rows(ui, self.sheets, rows, map, &mut self.sheet_rows);
        self.show_anims(ui);
        // Rigs and the selected rig's bones.
        let rows: Vec<Row> = self
            .doc
            .rigs
            .values()
            .map(|r| (None, key_of(&["rg", &r.id]), RowItem::new(r.name.clone())))
            .collect();
        let map = self
            .doc
            .rigs
            .keys()
            .map(|id| (key_of(&["rg", id]), id.clone()))
            .collect();
        show_rows(ui, self.rigs, rows, map, &mut self.rig_rows);
        self.show_bones(ui);
    }

    fn show_rules(&mut self, ui: &mut Ui) {
        let terrain = self.sel_terrain.as_ref().and_then(|tr| {
            self.sel_tileset
                .as_ref()
                .and_then(|ts| self.doc.tilesets.get(ts))
                .and_then(|ts| ts.terrains.get(tr))
        });
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(t) = terrain {
            for mask in blob47() {
                let k = key_of(&["rule", &t.id, &mask.to_string()]);
                let tile = t
                    .rules
                    .get(&mask)
                    .map_or(forge_ui::tr!("unset").to_string(), |x| {
                        forge_ui::trf!("tile {x}", x)
                    });
                rows.push((
                    None,
                    k,
                    RowItem::new(format!("{} \u{2192} {tile}", mask_name(mask)))
                        .muted(!t.rules.contains_key(&mask)),
                ));
                map.insert(k, mask);
            }
        }
        show_rows(ui, self.rules, rows, map, &mut self.rule_rows);
    }

    fn show_anims(&mut self, ui: &mut Ui) {
        let sheet = self.sel_sheet.as_ref().and_then(|s| self.doc.sheets.get(s));
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(s) = sheet {
            for a in s.anims.values() {
                let k = key_of(&["an", &s.id, &a.id]);
                rows.push((
                    None,
                    k,
                    RowItem::new(forge_ui::trf!(
                        "{name} \u{2014} {frames_count} frames at {fps} fps{looping}",
                        name = a.name,
                        frames_count = a.frames.len(),
                        fps = a.fps,
                        looping = if a.looping {
                            forge_ui::tr!(", looping")
                        } else {
                            ""
                        }
                    )),
                ));
                map.insert(k, a.id.clone());
            }
        }
        show_rows(ui, self.anims, rows, map, &mut self.anim_rows);
    }

    fn show_bones(&mut self, ui: &mut Ui) {
        let rig = self.sel_rig.as_ref().and_then(|r| self.doc.rigs.get(r));
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        if let Some(r) = rig {
            // Parents before children (depth-first from the roots); bones in a loop are
            // listed at the top level so they can be fixed.
            let mut placed = std::collections::BTreeSet::new();
            let mut stack: Vec<(Option<u64>, String)> = r
                .bones
                .values()
                .filter(|b| b.parent.as_ref().is_none_or(|p| !r.bones.contains_key(p)))
                .map(|b| (None, b.id.clone()))
                .collect();
            stack.reverse();
            while let Some((parent, id)) = stack.pop() {
                if !placed.insert(id.clone()) {
                    continue;
                }
                let Some(b) = r.bones.get(&id) else { continue };
                let k = key_of(&["bone", &r.id, &id]);
                rows.push((parent, k, RowItem::new(b.name.clone())));
                map.insert(k, id.clone());
                let mut kids: Vec<&s2::Bone> = r
                    .bones
                    .values()
                    .filter(|c| c.parent.as_deref() == Some(id.as_str()))
                    .collect();
                kids.reverse();
                for c in kids {
                    stack.push((Some(k), c.id.clone()));
                }
            }
            for b in r.bones.values() {
                if !placed.contains(&b.id) {
                    let k = key_of(&["bone", &r.id, &b.id]);
                    rows.push((
                        None,
                        k,
                        RowItem::new(forge_ui::trf!("{name} (in a parent loop)", name = b.name))
                            .muted(true),
                    ));
                    map.insert(k, b.id.clone());
                }
            }
        }
        show_rows(ui, self.bones, rows, map, &mut self.bone_rows);
    }

    /// Push the selection's data into the canvas, palette, preview and rig view, and the
    /// fields' signals.
    fn show_views(&mut self, ui: &mut Ui, sv: &EditorServices) {
        self.show_canvas(ui);
        self.show_side_views(ui, sv);
    }

    /// The canvas shows the selected layer (shared with the model, not copied).
    fn show_canvas(&mut self, ui: &mut Ui) {
        {
            let mut v = self.canvas_view.borrow_mut();
            let (layer, tileset, label) = match &self.sel_layer {
                Some((mp, l)) => {
                    let tm = self.doc.tilemaps.get(mp);
                    let layer = tm.and_then(|t| t.layers.get(l)).cloned();
                    let tileset = tm.and_then(|t| self.doc.tilesets.get(&t.tileset)).cloned();
                    let label = match (tm, &layer) {
                        (Some(t), Some(ly)) => forge_ui::trf!(
                            "Tile map {map}, layer {layer}",
                            map = t.name,
                            layer = ly.name
                        ),
                        _ => String::new(),
                    };
                    (layer, tileset, label)
                }
                None => (None, None, String::new()),
            };
            v.layer = layer;
            v.tileset = tileset;
            v.label = label;
            if self.stroke.is_none() {
                v.stroke = Stroke::default();
            }
        }
        ui.invalidate(self.canvas, Dirty::PAINT | Dirty::A11Y);
    }

    fn show_side_views(&mut self, ui: &mut Ui, sv: &EditorServices) {
        // Palette: the selected tile set (or the map's).
        let ts = self.sel_tileset.clone().or_else(|| {
            self.sel_layer
                .as_ref()
                .and_then(|(m, _)| self.doc.tilemaps.get(m))
                .map(|t| t.tileset.clone())
        });
        let tileset = ts.and_then(|t| self.doc.tilesets.get(&t));
        {
            let mut p = self.palette_view.borrow_mut();
            p.count = tileset.map_or(0, |t| u32::try_from(t.count).unwrap_or(0));
            p.columns = tileset.map_or(0, |t| u32::try_from(t.columns).unwrap_or(0));
            p.label = tileset.map_or(forge_ui::tr!("Tile palette").into(), |t| {
                forge_ui::trf!("Tile palette: {tileset}", tileset = t.name)
            });
            let rule_tile = self.sel_rule.and_then(|m| {
                self.sel_terrain
                    .as_ref()
                    .and_then(|tr| tileset.and_then(|t| t.terrains.get(tr)))
                    .and_then(|t| t.rules.get(&m).copied())
            });
            p.selected = match (self.palette_mode.get(ui.rt()), &self.brush) {
                (1, _) => rule_tile,
                (_, Some(Brush::Tile(t))) => Some(*t),
                _ => None,
            };
        }
        ui.invalidate(self.palette, Dirty::PAINT | Dirty::A11Y);
        for (_, sig, k) in &self.ts_fields {
            let v = tileset.map_or(0, |t| match *k {
                "columns" => t.columns,
                "count" => t.count,
                "tile_w" => t.tile_w,
                _ => t.tile_h,
            });
            if sig.get(ui.rt()) != i128::from(v) {
                sig.set(ui.rt_mut(), i128::from(v));
            }
        }
        // Sprite sheet preview and fields.
        let sheet = self.sel_sheet.as_ref().and_then(|s| self.doc.sheets.get(s));
        {
            let mut v = self.sheet_view.borrow_mut();
            v.slice = sheet.map(|s| s.slice);
            v.frames = sheet
                .map(|s| sv.scene2d.slice(&s.slice))
                .unwrap_or_default();
            v.highlight = sheet
                .and_then(|s| self.sel_anim.as_ref().and_then(|a| s.anims.get(a)))
                .map(|a| a.frames.clone())
                .unwrap_or_default();
            v.label = sheet.map_or(String::new(), |s| {
                forge_ui::trf!(
                    "Sprite sheet {name}: {n} frames",
                    name = s.name,
                    n = v.frames.len()
                )
            });
        }
        ui.invalidate(self.preview, Dirty::PAINT | Dirty::A11Y);
        for (_, sig, k) in &self.slice_fields {
            let v = sheet.map_or(0, |s| slice_field(&s.slice, k));
            if sig.get(ui.rt()) != i128::from(v) {
                sig.set(ui.rt_mut(), i128::from(v));
            }
        }
        let anim = sheet.and_then(|s| self.sel_anim.as_ref().and_then(|a| s.anims.get(a)));
        let (fps, looping) = anim.map_or((12.0, true), |a| (a.fps, a.looping));
        if self.fps.get(ui.rt()).to_bits() != fps.to_bits() {
            self.fps.set(ui.rt_mut(), fps);
        }
        if self.looping.get(ui.rt()) != looping {
            self.looping.set(ui.rt_mut(), looping);
        }
        // Rig view and bone fields.
        let rig = self.sel_rig.as_ref().and_then(|r| self.doc.rigs.get(r));
        {
            let mut d = self.rig_data.borrow_mut();
            d.pose = rig.map(|r| sv.scene2d.pose(r)).unwrap_or_default();
            d.selected = self.sel_bone.clone();
            d.name = rig.map_or(String::new(), |r| r.name.clone());
        }
        ui.invalidate(self.rig_view, Dirty::PAINT | Dirty::A11Y);
        if self.bone_drag.is_none() {
            let bone = rig.and_then(|r| self.sel_bone.as_ref().and_then(|b| r.bones.get(b)));
            for (_, sig, k) in &self.bone_fields {
                let v = bone.map_or(0.0, |b| match *k {
                    "x" => b.x,
                    "y" => b.y,
                    "rot" => b.rot,
                    _ => b.len,
                });
                if sig.get(ui.rt()).to_bits() != v.to_bits() {
                    sig.set(ui.rt_mut(), v);
                }
            }
            let z = i128::from(bone.map_or(0, |b| b.z));
            if self.bone_z.get(ui.rt()) != z {
                self.bone_z.set(ui.rt_mut(), z);
            }
            let sp = bone.map_or(String::new(), |b| b.sprite.clone());
            if self.bone_sprite.get(ui.rt()) != sp {
                self.bone_sprite.set(ui.rt_mut(), sp);
            }
        }
    }

    /// A stroke step: paint the cells into the stroke, then send it.
    fn paint(
        &mut self,
        ui: &mut Ui,
        session: &mut SessionState,
        cmd: &CommandEmitter,
        e: &PaintCells,
    ) {
        if e.phase == StrokePhase::End {
            if let Some(st) = self.stroke.take()
                && let Some(g) = st.gesture
            {
                g.commit();
            }
            return;
        }
        let Some((map, layer_ix)) = self.sel_layer.clone() else {
            refuse(
                session,
                forge_ui::tr!("Nothing to paint on"),
                forge_ui::tr!("Pick a tile map layer in the Maps list first."),
            );
            return;
        };
        let cell = if e.erase {
            MapCell::Empty
        } else {
            match &self.brush {
                Some(Brush::Tile(t)) => MapCell::Tile(*t),
                Some(Brush::Terrain(t)) => MapCell::Terrain(t.clone()),
                None => {
                    refuse(
                        session,
                        forge_ui::tr!("No brush"),
                        forge_ui::tr!(
                            "Pick a tile in the palette or a terrain in the tile sets list."
                        ),
                    );
                    return;
                }
            }
        };
        // A handle on the shared layer (no cells are copied; the stroke copies only the
        // chunks it paints). Dropped at the end of the step, so the model stays the only
        // holder when the mirror's echo is applied in place.
        let Some(layer) = self
            .doc
            .tilemaps
            .get(&map)
            .and_then(|t| t.layers.get(&layer_ix))
            .map(Arc::clone)
        else {
            return;
        };
        if matches!(e.phase, StrokePhase::Begin | StrokePhase::Single) {
            self.canvas_view.borrow_mut().stroke = Stroke::default();
            if let Some(st) = self.stroke.take()
                && let Some(g) = st.gesture
            {
                g.commit();
            }
        }
        // A step replaces one still held back (same frame): that one's chunks stay unsent
        // and go with this step. Otherwise every step so far went out: start from here.
        let holding = self
            .stroke
            .as_ref()
            .and_then(|st| st.gesture.as_ref())
            .is_some_and(Gesture::is_holding);
        if !holding {
            self.canvas_view.borrow_mut().stroke.mark_sent();
        }
        let changed = {
            let mut v = self.canvas_view.borrow_mut();
            let mut any = false;
            for (x, y) in &e.cells {
                any |= v.stroke.paint(&layer, *x, *y, cell.clone());
            }
            any
        };
        let label = if e.erase {
            forge_ui::tr!("Erase tiles")
        } else {
            forge_ui::tr!("Paint tiles")
        };
        let cmds = if self.fault_resend_stroke {
            // The positive control: every step re-sends the whole stroke so far.
            self.canvas_view.borrow().stroke.commands(&map, layer_ix)
        } else {
            self.canvas_view
                .borrow()
                .stroke
                .unsent_commands(&map, layer_ix)
        };
        match e.phase {
            StrokePhase::Single => {
                if changed {
                    cmd.emit_all(label, cmds);
                }
            }
            StrokePhase::Begin => {
                let gesture = if self.fault_no_gesture {
                    cmd.emit_all(label, cmds);
                    None
                } else {
                    let mut g = cmd.gesture(label);
                    g.update_all(cmds);
                    Some(g)
                };
                self.stroke = Some(StrokeState {
                    map,
                    layer: layer_ix,
                    gesture,
                });
            }
            StrokePhase::Move => {
                if changed && let Some(st) = self.stroke.as_mut() {
                    if st.map != map || st.layer != layer_ix {
                        return;
                    }
                    match st.gesture.as_mut() {
                        Some(g) => g.update_all(cmds),
                        // The positive control: every step its own transaction.
                        None => {
                            cmd.emit_all(label, cmds);
                        }
                    }
                }
            }
            StrokePhase::End => {}
        }
        ui.invalidate(self.canvas, Dirty::PAINT | Dirty::A11Y);
    }
}

/// "N E S W + NE" for a mask.
pub fn mask_name(mask: u8) -> String {
    let names = [
        (s2::N, "N"),
        (s2::E, "E"),
        (s2::S, "S"),
        (s2::W, "W"),
        (s2::NE, "NE"),
        (s2::SE, "SE"),
        (s2::SW, "SW"),
        (s2::NW, "NW"),
    ];
    let v: Vec<&str> = names
        .iter()
        .filter(|(b, _)| mask & b != 0)
        .map(|(_, n)| *n)
        .collect();
    if v.is_empty() {
        forge_ui::trf!("mask {mask}: isolated", mask = format!("{mask:3}"))
    } else {
        forge_ui::trf!(
            "mask {mask}: {sides}",
            mask = format!("{mask:3}"),
            sides = v.join(" ")
        )
    }
}

fn slice_field(s: &s2::SheetSlice, k: &str) -> i64 {
    match k {
        "image_w" => s.image_w,
        "image_h" => s.image_h,
        "cell_w" => s.cell_w,
        "cell_h" => s.cell_h,
        "offset_x" => s.offset_x,
        "offset_y" => s.offset_y,
        "spacing_x" => s.spacing_x,
        _ => s.spacing_y,
    }
}

fn page(
    pb: &mut forge_editor::panel_rt::PanelBuilder,
    key: &'static str,
    label: &str,
    space: f32,
) -> Result<WidgetId, forge_ui::UiError> {
    pb.b.add(
        pb.parent,
        key,
        NodeStyle::column(space).grow(1.0).padding(space),
        Container::new(Role::Group).labelled(label),
    )
}

fn toolbar(
    pb: &mut forge_editor::panel_rt::PanelBuilder,
    parent: WidgetId,
    label: &str,
    space: f32,
) -> Result<WidgetId, forge_ui::UiError> {
    pb.b.add(
        parent,
        "bar",
        NodeStyle::row(space).wrap(),
        Container::new(Role::Toolbar).labelled(label),
    )
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space[1];
        // The sync step fills the lists from the mirror: run it right after the build.
        pb.want_turn();
        let sv = pb.services();
        let faults = sv.faults.clone();
        let selected_tab = pb.b.signal(0usize);
        let page_ids: Vec<WidgetId> = ["tiles", "sheet", "rig"]
            .iter()
            .map(|k| pb.parent.child(&Key::Str((*k).into())))
            .collect();
        let tab_names: Vec<&str> = TABS.iter().map(|t| forge_ui::l10n::tr(t)).collect();
        let mut tabs = Tabs::new(&tab_names, selected_tab);
        tabs.set_pages(page_ids.clone());
        pb.b.add(pb.parent, "tabs", NodeStyle::leaf(), tabs)?;
        let nothing = pb.b.add(
            pb.parent,
            "nothing",
            NodeStyle::leaf(),
            forge_ui::widgets::EmptyState::new(forge_ui::tr!(
                "No tile sets, maps, sprite sheets or rigs yet: create one with the buttons on each page."
            )),
        )?;

        // ---- tile palette page -----------------------------------------------------------
        let tp = page(pb, "tiles", forge_ui::tr!("Tile palette"), space)?;
        let bar = toolbar(pb, tp, forge_ui::tr!("Tile actions"), space)?;
        let new_ts = pb.b.add(
            bar,
            "new_tileset",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New tile set")),
        )?;
        let add_terrain = pb.b.add(
            bar,
            "add_terrain",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add terrain")),
        )?;
        let template = pb.b.add(
            bar,
            "blob_template",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Blob template")),
        )?;
        let new_map = pb.b.add(
            bar,
            "new_map",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New tile map")),
        )?;
        let add_layer = pb.b.add(
            bar,
            "add_layer",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add layer")),
        )?;
        let remove_tiles = pb.b.add(
            bar,
            "remove",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Remove selected")),
        )?;
        let palette_mode = pb.b.signal(0usize);
        pb.b.add(
            bar,
            "mode",
            NodeStyle::leaf(),
            RadioGroup::new(
                forge_ui::tr!("Palette click"),
                &[forge_ui::tr!("Pick brush"), forge_ui::tr!("Assign to rule")],
                palette_mode,
            ),
        )?;
        let mode_relay = pb.b.add(
            bar,
            "mode.relay",
            NodeStyle::leaf(),
            SignalRelay::new(palette_mode.any()),
        )?;
        let body = pb.b.add(
            tp,
            "body",
            NodeStyle::row(space).grow(1.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Tile editing")),
        )?;
        let side = pb.b.add(
            body,
            "side",
            NodeStyle::column(space).width(260.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Tile sets and maps")),
        )?;
        let tiles = pb.b.add(
            side,
            "tilesets",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 80.0),
            VirtualTree::tree(forge_ui::tr!("Tile sets")).read_only().single_select(),
        )?;
        let mut ts_fields = Vec::new();
        let tsf = pb.b.add(
            side,
            "tileset_fields",
            NodeStyle::grid(2, space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Tile set")),
        )?;
        for (k, label) in [
            ("tile_w", forge_ui::tr!("Tile width")),
            ("tile_h", forge_ui::tr!("Tile height")),
            ("columns", forge_ui::tr!("Columns")),
            ("count", forge_ui::tr!("Tiles")),
        ] {
            let sig = pb.b.signal(0i128);
            let id = pb.b.add(
                tsf,
                k,
                NodeStyle::leaf(),
                IntegerField::new(sig, label).range(1, 65_536),
            )?;
            ts_fields.push((id, sig, k));
        }
        let maps = pb.b.add(
            side,
            "maps",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 80.0),
            VirtualTree::tree(forge_ui::tr!("Tile maps")).read_only().single_select(),
        )?;
        let rules = pb.b.add(
            side,
            "rules",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 80.0),
            VirtualTree::list(forge_ui::tr!("Autotile rules"))
                .read_only()
                .single_select(),
        )?;
        let main = pb.b.add(
            body,
            "main",
            NodeStyle::column(space).grow(1.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Canvas and palette")),
        )?;
        let canvas_view = Rc::new(RefCell::new(CanvasView::default()));
        let mut canvas_w = TileCanvas::new(canvas_view.clone(), sv.scene2d.clone());
        if faults.tilemap_paint_every_cell() {
            canvas_w = canvas_w.with_fault_paint_all();
        }
        let canvas = pb.b.add(
            main,
            "canvas",
            NodeStyle::leaf().grow(1.0).min_size(160.0, 160.0),
            canvas_w,
        )?;
        let palette_view = Rc::new(RefCell::new(PaletteView::default()));
        let palette = pb.b.add(
            main,
            "palette",
            NodeStyle::leaf().height(120.0),
            TilePalette::new(palette_view.clone()),
        )?;

        // ---- sprite sheet page ------------------------------------------------------------
        let sp = page(pb, "sheet", forge_ui::tr!("Sprite sheet"), space)?;
        pb.b.hide(sp, true);
        let bar = toolbar(pb, sp, forge_ui::tr!("Sprite sheet actions"), space)?;
        let image = pb.b.signal(String::new());
        let image_field = pb.b.add(
            bar,
            "image",
            NodeStyle::leaf().width(220.0),
            TextField::new(image, forge_ui::tr!("Sheet image")).placeholder(forge_ui::tr!("sprites/hero.png")),
        )?;
        let new_sheet = pb.b.add(
            bar,
            "new_sheet",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New sheet")),
        )?;
        let new_anim = pb.b.add(
            bar,
            "new_anim",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New animation")),
        )?;
        let drop_frame = pb.b.add(
            bar,
            "drop_frame",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Remove last frame")),
        )?;
        let remove_sheet = pb.b.add(
            bar,
            "remove",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Remove selected")),
        )?;
        let body = pb.b.add(
            sp,
            "body",
            NodeStyle::row(space).grow(1.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Sheet editing")),
        )?;
        let side = pb.b.add(
            body,
            "side",
            NodeStyle::column(space).width(260.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Sheets and animations")),
        )?;
        let sheets = pb.b.add(
            side,
            "sheets",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 60.0),
            VirtualTree::list(forge_ui::tr!("Sprite sheets"))
                .read_only()
                .single_select(),
        )?;
        let slf = pb.b.add(
            side,
            "slice",
            NodeStyle::grid(2, space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Slicing")),
        )?;
        let mut slice_fields = Vec::new();
        for (k, label) in SLICE_FIELDS {
            let sig = pb.b.signal(0i128);
            let id = pb.b.add(
                slf,
                k,
                NodeStyle::leaf(),
                IntegerField::new(sig, forge_ui::l10n::tr(label)).range(0, 65_536).unit("px"),
            )?;
            slice_fields.push((id, sig, k));
        }
        let anims = pb.b.add(
            side,
            "anims",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 60.0),
            VirtualTree::list(forge_ui::tr!("Animations")).single_select(),
        )?;
        let af = pb.b.add(
            side,
            "anim_fields",
            NodeStyle::row(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Animation")),
        )?;
        let fps = pb.b.signal(12.0f64);
        let fps_field = pb.b.add(
            af,
            "fps",
            NodeStyle::leaf(),
            NumericField::new(fps, forge_ui::tr!("Frames per second"))
                .range(0.1, 240.0)
                .unit("fps"),
        )?;
        let looping = pb.b.signal(true);
        pb.b.add(
            af,
            "looping",
            NodeStyle::leaf(),
            Checkbox::new(looping, forge_ui::tr!("Looping")),
        )?;
        let loop_relay = pb.b.add(
            af,
            "looping.relay",
            NodeStyle::leaf(),
            SignalRelay::new(looping.any()),
        )?;
        let sheet_view = Rc::new(RefCell::new(SheetView::default()));
        let preview = pb.b.add(
            body,
            "preview",
            NodeStyle::leaf().grow(1.0).min_size(160.0, 160.0),
            SheetPreview::new(sheet_view.clone()),
        )?;

        // ---- rig page ---------------------------------------------------------------------
        let rp = page(pb, "rig", forge_ui::tr!("2D rig"), space)?;
        pb.b.hide(rp, true);
        let bar = toolbar(pb, rp, forge_ui::tr!("Rig actions"), space)?;
        let new_rig =
            pb.b.add(bar, "new_rig", NodeStyle::leaf(), Button::new(forge_ui::tr!("New rig")))?;
        let add_bone =
            pb.b.add(bar, "add_bone", NodeStyle::leaf(), Button::new(forge_ui::tr!("Add bone")))?;
        let remove_bone = pb.b.add(
            bar,
            "remove_bone",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Remove bone")),
        )?;
        let body = pb.b.add(
            rp,
            "body",
            NodeStyle::row(space).grow(1.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Rig editing")),
        )?;
        let side = pb.b.add(
            body,
            "side",
            NodeStyle::column(space).width(260.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("Rigs and bones")),
        )?;
        let rigs = pb.b.add(
            side,
            "rigs",
            NodeStyle::leaf().min_size(0.0, 60.0),
            VirtualTree::list(forge_ui::tr!("Rigs")).read_only().single_select(),
        )?;
        let bones = pb.b.add(
            side,
            "bones",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 80.0),
            VirtualTree::tree(forge_ui::tr!("Bones")).single_select(),
        )?;
        let bf = pb.b.add(
            side,
            "bone_fields",
            NodeStyle::grid(2, space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Bone")),
        )?;
        let mut bone_fields = Vec::new();
        for (k, label, unit) in BONE_FIELDS {
            let sig = pb.b.signal(0.0f64);
            let id = pb.b.add(
                bf,
                k,
                NodeStyle::leaf(),
                NumericField::new(sig, forge_ui::l10n::tr(label)).unit(unit).decimals(2),
            )?;
            bone_fields.push((id, sig, k));
        }
        let bone_z = pb.b.signal(0i128);
        let z_field = pb.b.add(
            bf,
            "z",
            NodeStyle::leaf(),
            IntegerField::new(bone_z, forge_ui::tr!("Draw order")).range(-10_000, 10_000),
        )?;
        let bone_sprite = pb.b.signal(String::new());
        let sprite_field = pb.b.add(
            bf,
            "sprite",
            NodeStyle::leaf(),
            TextField::new(bone_sprite, forge_ui::tr!("Sprite (sheet:frame)")),
        )?;
        let rig_data = Rc::new(RefCell::new(RigViewData::default()));
        let rig_view = pb.b.add(
            body,
            "view",
            NodeStyle::leaf().grow(1.0).min_size(160.0, 160.0),
            RigView::new(rig_data.clone()),
        )?;

        // ---- footer -------------------------------------------------------------------------
        let status =
            pb.b.signal(forge_ui::tr!("Pick a tile set or map, a sheet, or a rig.").to_string());
        pb.b.add(
            pb.parent,
            "status",
            NodeStyle::leaf().padding(space),
            Label::new(status).kind(LabelKind::Warning).wrapping(),
        )?;
        let problems = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "problems",
            NodeStyle::leaf().padding(space),
            Label::new(problems).kind(LabelKind::Warning).wrapping(),
        )?;
        pb.b.add(
            pb.parent,
            "backend",
            NodeStyle::leaf().padding(space),
            Label::new(forge_ui::l10n::tr_str(&sv.scene2d.backend().note).into_owned())
                .kind(LabelKind::Small)
                .wrapping(),
        )?;

        let ed = Rc::new(RefCell::new(Ed {
            doc: Doc2d::default(),
            seen: u64::MAX,
            setting_seq: None,
            fault_full_reread: faults.d2_full_reread(),
            fault_resend_stroke: faults.d2_stroke_resends_all(),
            probe: faults.d2_model_probe().clone(),
            status,
            problems,
            nothing,
            tiles,
            tile_rows: Rows::default(),
            maps,
            map_rows: Rows::default(),
            rules,
            rule_rows: Rows::default(),
            canvas,
            canvas_view,
            palette,
            palette_view,
            palette_mode,
            ts_fields,
            brush: None,
            sel_tileset: None,
            sel_terrain: None,
            sel_layer: None,
            sel_rule: None,
            stroke: None,
            fault_no_gesture: faults.tile_paint_without_gesture(),
            sheets,
            sheet_rows: Rows::default(),
            anims,
            anim_rows: Rows::default(),
            preview,
            sheet_view,
            slice_fields,
            fps,
            looping,
            sel_sheet: None,
            sel_anim: None,
            rigs,
            rig_rows: Rows::default(),
            bones,
            bone_rows: Rows::default(),
            rig_view,
            rig_data,
            bone_fields,
            bone_z,
            bone_sprite,
            sel_rig: None,
            sel_bone: None,
            bone_drag: None,
        }));

        // ---- tile palette handlers --------------------------------------------------------
        let e = ed.clone();
        pb.on(new_ts, move |act, _: &Pressed| {
            let ed = e.borrow();
            let (id, cmds) = s2::new_tileset(&ed.doc, forge_ui::tr!("Tile set"));
            act.cmd.emit_all(forge_ui::tr!("New tile set"), cmds);
            ed.say(
                act.ui,
                forge_ui::trf!("Created tile set {id} (48 tiles of 16 px, terrain Ground).", id),
            );
        });
        let e = ed.clone();
        pb.on(add_terrain, move |act, _: &Pressed| {
            let ed = e.borrow();
            let Some(ts) = ed.sel_tileset.as_ref().and_then(|t| ed.doc.tilesets.get(t)) else {
                refuse(act.session, forge_ui::tr!("Add terrain"), forge_ui::tr!("Select a tile set first."));
                return;
            };
            let id = forge_editor::domain::unique_id("terrain", |s| ts.terrains.contains_key(s));
            act.cmd.emit(set(
                s2::key(TILESET, &ts.id, &format!("terrain.{id}.name")),
                Value::Text(forge_ui::trf!("Terrain {n}", n = ts.terrains.len() + 1)),
            ));
        });
        let e = ed.clone();
        pb.on(template, move |act, _: &Pressed| {
            let ed = e.borrow();
            let (Some(ts), Some(tr)) = (ed.sel_tileset.clone(), ed.sel_terrain.clone()) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Blob template"),
                    forge_ui::tr!("Select a terrain in the tile sets list first."),
                );
                return;
            };
            act.cmd.emit_all(
                forge_ui::tr!("Fill autotile rules from the blob template"),
                s2::blob_template(&ts, &tr, 0),
            );
            ed.say(
                act.ui,
                forge_ui::tr!("Filled all 47 rules: mask i shows tile i of the blob layout."),
            );
        });
        let e = ed.clone();
        pb.on(new_map, move |act, _: &Pressed| {
            let ed = e.borrow();
            let ts = ed
                .sel_tileset
                .clone()
                .or_else(|| ed.doc.tilesets.keys().next().cloned());
            let Some(ts) = ts else {
                refuse(
                    act.session,
                    forge_ui::tr!("New tile map"),
                    forge_ui::tr!("Create a tile set first: a map paints its tiles."),
                );
                return;
            };
            let (id, cmds) = s2::new_tilemap(&ed.doc, forge_ui::tr!("Tile map"), &ts);
            act.cmd.emit_all(forge_ui::tr!("New tile map"), cmds);
            ed.say(act.ui, forge_ui::trf!("Created tile map {id} over {ts}.", id, ts));
        });
        let e = ed.clone();
        pb.on(add_layer, move |act, _: &Pressed| {
            let ed = e.borrow();
            let Some((mp, _)) = ed.sel_layer.clone() else {
                refuse(act.session, forge_ui::tr!("Add layer"), forge_ui::tr!("Select a tile map first."));
                return;
            };
            let next = ed
                .doc
                .tilemaps
                .get(&mp)
                .and_then(|t| t.layers.keys().next_back().map(|l| l + 1))
                .unwrap_or(0);
            act.cmd.emit_all(
                forge_ui::tr!("Add layer"),
                vec![
                    set(
                        s2::key(TILEMAP, &mp, &format!("layer.{next}.name")),
                        Value::Text(forge_ui::trf!("Layer {next}", next)),
                    ),
                    set(
                        s2::key(TILEMAP, &mp, &format!("layer.{next}.visible")),
                        Value::Bool(true),
                    ),
                ],
            );
        });
        let e = ed.clone();
        pb.on(remove_tiles, move |act, _: &Pressed| {
            let ed = e.borrow();
            let tiles_sel = selected(act.ui, ed.tiles).and_then(|k| ed.tile_rows.get(k));
            let maps_sel = selected(act.ui, ed.maps).and_then(|k| ed.map_rows.get(k));
            let (label, prefix) = match (maps_sel, tiles_sel) {
                (Some(MapRow::Layer(m, l)), _) => {
                    (forge_ui::tr!("Remove layer"), format!("{TILEMAP}.{m}.layer.{l}"))
                }
                (Some(MapRow::Map(m)), _) => (forge_ui::tr!("Remove tile map"), format!("{TILEMAP}.{m}")),
                (None, Some(TileRow::Terrain(t, tr))) => {
                    (forge_ui::tr!("Remove terrain"), format!("{TILESET}.{t}.terrain.{tr}"))
                }
                (None, Some(TileRow::Tileset(t))) => (forge_ui::tr!("Remove tile set"), format!("{TILESET}.{t}")),
                (None, None) => {
                    refuse(
                        act.session,
                        forge_ui::tr!("Remove"),
                        forge_ui::tr!("Select a tile set, terrain, map or layer first."),
                    );
                    return;
                }
            };
            act.cmd.emit_all(label, clear_under(act.mirror, &prefix));
        });
        let e = ed.clone();
        pb.on(tiles, move |act, ev: &SelectionChanged| {
            let mut ed = e.borrow_mut();
            match ev.keys.first().and_then(|k| ed.tile_rows.get(*k)) {
                Some(TileRow::Tileset(t)) => {
                    ed.sel_tileset = Some(t);
                    ed.sel_terrain = None;
                }
                Some(TileRow::Terrain(t, tr)) => {
                    ed.sel_tileset = Some(t);
                    ed.brush = Some(Brush::Terrain(tr.clone()));
                    ed.sel_terrain = Some(tr.clone());
                    ed.say(act.ui, forge_ui::trf!("Brush: terrain {tr} (autotiled).", tr));
                }
                None => {}
            }
            ed.sel_rule = None;
            ed.show_rules(act.ui);
            ed.show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(maps, move |act, ev: &SelectionChanged| {
            let mut ed = e.borrow_mut();
            let layer = match ev.keys.first().and_then(|k| ed.map_rows.get(*k)) {
                Some(MapRow::Layer(m, l)) => Some((m, l)),
                Some(MapRow::Map(m)) => ed
                    .doc
                    .tilemaps
                    .get(&m)
                    .and_then(|t| t.layers.keys().next().map(|l| (m.clone(), *l))),
                None => None,
            };
            if layer.is_some() {
                ed.sel_layer = layer;
            }
            ed.show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(rules, move |act, ev: &SelectionChanged| {
            let mut ed = e.borrow_mut();
            ed.sel_rule = ev.keys.first().and_then(|k| ed.rule_rows.get(*k));
            ed.show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(mode_relay, move |act, _: &SignalChanged| {
            e.borrow_mut().show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(palette, move |act, ev: &TilePicked| {
            let mut ed = e.borrow_mut();
            if ed.palette_mode.get(act.ui.rt()) == 1 {
                let (Some(ts), Some(tr), Some(mask)) =
                    (ed.sel_tileset.clone(), ed.sel_terrain.clone(), ed.sel_rule)
                else {
                    refuse(
                        act.session,
                        forge_ui::tr!("Assign to rule"),
                        forge_ui::tr!("Select a terrain and one of its rules first."),
                    );
                    return;
                };
                act.cmd.emit(set(
                    s2::key(TILESET, &ts, &format!("terrain.{tr}.rule.{mask}")),
                    Value::Int(i64::from(ev.tile)),
                ));
                ed.say(
                    act.ui,
                    forge_ui::trf!("{mask_name} now shows tile {tile}.", mask_name = mask_name(mask), tile = ev.tile),
                );
            } else {
                ed.brush = Some(Brush::Tile(ev.tile));
                ed.say(act.ui, forge_ui::trf!("Brush: tile {tile}.", tile = ev.tile));
                ed.show_views(act.ui, act.services);
            }
        });
        for (id, _, k) in ed.borrow().ts_fields.iter() {
            let (e, k) = (ed.clone(), *k);
            pb.on(*id, move |act, ev: &IntegerCommitted| {
                let ed = e.borrow();
                let Some(ts) = ed.sel_tileset.clone() else {
                    refuse(act.session, forge_ui::tr!("Tile set"), forge_ui::tr!("Select a tile set first."));
                    return;
                };
                act.cmd.emit(set(
                    s2::key(TILESET, &ts, k),
                    Value::Int(i64::try_from(ev.value).unwrap_or(1)),
                ));
            });
        }
        let e = ed.clone();
        pb.on(canvas, move |act, ev: &PaintCells| {
            e.borrow_mut().paint(act.ui, act.session, act.cmd, ev);
        });

        // ---- sprite sheet handlers --------------------------------------------------------
        let e = ed.clone();
        pb.on(new_sheet, move |act, _: &Pressed| {
            let ed = e.borrow();
            let img = image.get(act.ui.rt());
            let img = if img.trim().is_empty() {
                "sprites/sheet.png".to_string()
            } else {
                img.trim().to_string()
            };
            let size = act.services.scene2d.image_size(&img).unwrap_or((256, 256));
            let name = img
                .rsplit('/')
                .next()
                .unwrap_or("sheet")
                .split('.')
                .next()
                .unwrap_or("sheet")
                .to_string();
            let (id, cmds) = s2::new_sheet(&ed.doc, &name, &img, size);
            act.cmd.emit_all(forge_ui::tr!("New sprite sheet"), cmds);
            ed.say(
                act.ui,
                forge_ui::trf!("Created sprite sheet {id} ({first}\u{d7}{second} px).", id, first = size.0, second = size.1),
            );
        });
        let e = ed.clone();
        pb.on(image_field, move |act, _: &Submitted| {
            // Enter in the image field creates the sheet too.
            let _ = &e;
            act.ui.raise(new_sheet, Pressed(new_sheet));
        });
        let e = ed.clone();
        pb.on(new_anim, move |act, _: &Pressed| {
            let ed = e.borrow();
            let Some(sh) = ed.sel_sheet.as_ref().and_then(|s| ed.doc.sheets.get(s)) else {
                refuse(act.session, forge_ui::tr!("New animation"), forge_ui::tr!("Select a sprite sheet first."));
                return;
            };
            let id = forge_editor::domain::unique_id("anim", |s| sh.anims.contains_key(s));
            let k = |f: &str| s2::key(SHEET, &sh.id, &format!("anim.{id}.{f}"));
            act.cmd.emit_all(
                forge_ui::tr!("New animation"),
                vec![
                    set(
                        k("name"),
                        Value::Text(forge_ui::trf!("Animation {n}", n = sh.anims.len() + 1)),
                    ),
                    set(k("frames"), Value::Text(String::new())),
                    set(k("fps"), Value::Float(12.0)),
                    set(k("looping"), Value::Bool(true)),
                ],
            );
            ed.say(
                act.ui,
                forge_ui::tr!("Pick frames in the preview to add them to the animation."),
            );
        });
        let e = ed.clone();
        pb.on(drop_frame, move |act, _: &Pressed| {
            let ed = e.borrow();
            let Some((sh, a)) = sel_anim(&ed) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Remove last frame"),
                    forge_ui::tr!("Select an animation first."),
                );
                return;
            };
            let mut f = a.frames.clone();
            if f.pop().is_some() {
                act.cmd.emit(set(
                    s2::key(SHEET, &sh, &format!("anim.{}.frames", a.id)),
                    Value::Text(s2::format_frames(&f)),
                ));
            }
        });
        let e = ed.clone();
        pb.on(remove_sheet, move |act, _: &Pressed| {
            let ed = e.borrow();
            if let Some((sh, a)) = sel_anim(&ed) {
                act.cmd.emit_all(
                    forge_ui::tr!("Remove animation"),
                    clear_under(act.mirror, &format!("{SHEET}.{sh}.anim.{}", a.id)),
                );
            } else if let Some(sh) = ed.sel_sheet.clone() {
                act.cmd.emit_all(
                    forge_ui::tr!("Remove sprite sheet"),
                    clear_under(act.mirror, &format!("{SHEET}.{sh}")),
                );
            } else {
                refuse(
                    act.session,
                    forge_ui::tr!("Remove"),
                    forge_ui::tr!("Select a sprite sheet or an animation first."),
                );
            }
        });
        let e = ed.clone();
        pb.on(sheets, move |act, ev: &SelectionChanged| {
            let mut ed = e.borrow_mut();
            if let Some(s) = ev.keys.first().and_then(|k| ed.sheet_rows.get(*k)) {
                ed.sel_sheet = Some(s);
                ed.sel_anim = None;
            }
            ed.show_anims(act.ui);
            ed.show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(anims, move |act, ev: &SelectionChanged| {
            let mut ed = e.borrow_mut();
            ed.sel_anim = ev.keys.first().and_then(|k| ed.anim_rows.get(*k));
            ed.show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(anims, move |act, ev: &RowRenamed| {
            let ed = e.borrow();
            let (Some(sh), Some(a)) = (ed.sel_sheet.clone(), ed.anim_rows.get(ev.key)) else {
                return;
            };
            act.cmd.emit(set(
                s2::key(SHEET, &sh, &format!("anim.{a}.name")),
                Value::Text(ev.name.clone()),
            ));
        });
        for (id, _, k) in ed.borrow().slice_fields.iter() {
            let (e, k) = (ed.clone(), *k);
            pb.on(*id, move |act, ev: &IntegerCommitted| {
                let ed = e.borrow();
                let Some(sh) = ed.sel_sheet.clone() else {
                    refuse(act.session, forge_ui::tr!("Slicing"), forge_ui::tr!("Select a sprite sheet first."));
                    return;
                };
                act.cmd.emit(set(
                    s2::key(SHEET, &sh, k),
                    Value::Int(i64::try_from(ev.value).unwrap_or(0)),
                ));
            });
        }
        let e = ed.clone();
        pb.on(fps_field, move |act, ev: &NumericCommitted| {
            let ed = e.borrow();
            let Some((sh, a)) = sel_anim(&ed) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Frames per second"),
                    forge_ui::tr!("Select an animation first."),
                );
                return;
            };
            act.cmd.emit(set(
                s2::key(SHEET, &sh, &format!("anim.{}.fps", a.id)),
                Value::Float(ev.value),
            ));
        });
        let e = ed.clone();
        pb.on(loop_relay, move |act, _: &SignalChanged| {
            let ed = e.borrow();
            let want = ed.looping.get(act.ui.rt());
            if let Some((sh, a)) = sel_anim(&ed)
                && a.looping != want
            {
                act.cmd.emit(set(
                    s2::key(SHEET, &sh, &format!("anim.{}.looping", a.id)),
                    Value::Bool(want),
                ));
            }
        });
        let e = ed.clone();
        pb.on(preview, move |act, ev: &FramePicked| {
            let ed = e.borrow();
            let Some((sh, a)) = sel_anim(&ed) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Add frame"),
                    forge_ui::tr!("Select (or create) an animation first."),
                );
                return;
            };
            let mut f = a.frames.clone();
            f.push(ev.frame);
            act.cmd.emit(set(
                s2::key(SHEET, &sh, &format!("anim.{}.frames", a.id)),
                Value::Text(s2::format_frames(&f)),
            ));
            ed.say(
                act.ui,
                forge_ui::trf!("{name}: frames {frames}", name = a.name, frames = s2::format_frames(&f)),
            );
        });

        // ---- rig handlers -----------------------------------------------------------------
        let e = ed.clone();
        pb.on(new_rig, move |act, _: &Pressed| {
            let ed = e.borrow();
            let (id, cmds) = s2::new_rig(&ed.doc, forge_ui::tr!("Rig"));
            act.cmd.emit_all(forge_ui::tr!("New rig"), cmds);
            ed.say(act.ui, forge_ui::trf!("Created rig {id} with a root bone.", id));
        });
        let e = ed.clone();
        pb.on(add_bone, move |act, _: &Pressed| {
            let ed = e.borrow();
            let Some(rig) = ed.sel_rig.as_ref().and_then(|r| ed.doc.rigs.get(r)) else {
                refuse(act.session, forge_ui::tr!("Add bone"), forge_ui::tr!("Select a rig first."));
                return;
            };
            let (id, cmds) = s2::new_bone(rig, forge_ui::tr!("Bone"), ed.sel_bone.as_deref());
            act.cmd.emit_all(forge_ui::tr!("Add bone"), cmds);
            ed.say(act.ui, forge_ui::trf!("Added bone {id}.", id));
        });
        let e = ed.clone();
        let remove = move |act: &mut forge_editor::panel_rt::PanelAct, bone: Option<String>| {
            let ed = e.borrow();
            let (Some(rig), Some(b)) = (ed.sel_rig.as_ref().and_then(|r| ed.doc.rigs.get(r)), bone)
            else {
                refuse(act.session, forge_ui::tr!("Remove bone"), forge_ui::tr!("Select a bone first."));
                return;
            };
            let mut cmds = Vec::new();
            for id in rig.subtree(&b) {
                cmds.extend(clear_under(
                    act.mirror,
                    &format!("{RIG}.{}.bone.{id}", rig.id),
                ));
            }
            act.cmd.emit_all(forge_ui::tr!("Remove bone"), cmds);
        };
        let remove = Rc::new(remove);
        let (e, r2) = (ed.clone(), remove.clone());
        pb.on(remove_bone, move |act, _: &Pressed| {
            let b = e.borrow().sel_bone.clone();
            r2(act, b);
        });
        let (e, r2) = (ed.clone(), remove);
        pb.on(bones, move |act, ev: &RowsDeleteRequested| {
            let b = ev.keys.first().and_then(|k| e.borrow().bone_rows.get(*k));
            r2(act, b);
        });
        let e = ed.clone();
        pb.on(rigs, move |act, ev: &SelectionChanged| {
            let mut ed = e.borrow_mut();
            if let Some(r) = ev.keys.first().and_then(|k| ed.rig_rows.get(*k)) {
                ed.sel_rig = Some(r);
                ed.sel_bone = None;
            }
            ed.show_bones(act.ui);
            ed.show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(bones, move |act, ev: &SelectionChanged| {
            let mut ed = e.borrow_mut();
            ed.sel_bone = ev.keys.first().and_then(|k| ed.bone_rows.get(*k));
            ed.show_views(act.ui, act.services);
        });
        let e = ed.clone();
        pb.on(bones, move |act, ev: &RowRenamed| {
            let ed = e.borrow();
            let (Some(rig), Some(b)) = (ed.sel_rig.clone(), ed.bone_rows.get(ev.key)) else {
                return;
            };
            act.cmd.emit(set(
                s2::key(RIG, &rig, &format!("bone.{b}.name")),
                Value::Text(ev.name.clone()),
            ));
        });
        let e = ed.clone();
        pb.on(bones, move |act, ev: &RowsDropped| {
            let ed = e.borrow();
            let Some(rig) = ed.sel_rig.as_ref().and_then(|r| ed.doc.rigs.get(r)) else {
                return;
            };
            let parent = ev.target.parent.and_then(|k| ed.bone_rows.get(k));
            let mut cmds = Vec::new();
            for k in &ev.keys {
                let Some(b) = ed.bone_rows.get(*k) else {
                    continue;
                };
                if rig.would_cycle(&b, parent.as_deref()) {
                    refuse(
                        act.session,
                        forge_ui::tr!("Move bone"),
                        &forge_ui::trf!("Moving {b} under {value} would make a bone its own ancestor.", b, value = parent.as_deref().unwrap_or(forge_ui::tr!("the root"))),
                    );
                    return;
                }
                let key = s2::key(RIG, &rig.id, &format!("bone.{b}.parent"));
                cmds.push(match &parent {
                    Some(p) => set(key, Value::Text(p.clone())),
                    None => clear(key),
                });
            }
            act.cmd.emit_all(forge_ui::tr!("Move bone"), cmds);
        });
        for (id, _, k) in ed.borrow().bone_fields.iter() {
            let (e, k) = (ed.clone(), *k);
            pb.on(*id, move |act, ev: &NumericCommitted| {
                let ed = e.borrow();
                let (Some(r), Some(b)) = (ed.sel_rig.clone(), ed.sel_bone.clone()) else {
                    refuse(act.session, forge_ui::tr!("Bone"), forge_ui::tr!("Select a bone first."));
                    return;
                };
                act.cmd.emit(set(
                    s2::key(RIG, &r, &format!("bone.{b}.{k}")),
                    Value::Float(ev.value),
                ));
            });
        }
        let e = ed.clone();
        pb.on(z_field, move |act, ev: &IntegerCommitted| {
            let ed = e.borrow();
            let (Some(r), Some(b)) = (ed.sel_rig.clone(), ed.sel_bone.clone()) else {
                return;
            };
            act.cmd.emit(set(
                s2::key(RIG, &r, &format!("bone.{b}.z")),
                Value::Int(i64::try_from(ev.value).unwrap_or(0)),
            ));
        });
        let e = ed.clone();
        pb.on(sprite_field, move |act, ev: &Submitted| {
            let ed = e.borrow();
            let (Some(r), Some(b)) = (ed.sel_rig.clone(), ed.sel_bone.clone()) else {
                return;
            };
            let key = s2::key(RIG, &r, &format!("bone.{b}.sprite"));
            act.cmd.emit(if ev.text.trim().is_empty() {
                clear(key)
            } else {
                set(key, Value::Text(ev.text.trim().to_string()))
            });
        });
        let e = ed.clone();
        pb.on(rig_view, move |act, ev: &BoneDrag| {
            let mut ed = e.borrow_mut();
            let Some(rig) = ed
                .sel_rig
                .as_ref()
                .and_then(|r| ed.doc.rigs.get(r))
                .cloned()
            else {
                return;
            };
            match ev.phase {
                StrokePhase::End => {
                    if let Some(g) = ed.bone_drag.take() {
                        g.commit();
                    }
                }
                StrokePhase::Begin | StrokePhase::Move | StrokePhase::Single => {
                    let Some(b) = rig.bones.get(&ev.bone) else {
                        return;
                    };
                    let parent_angle = b
                        .parent
                        .as_ref()
                        .and_then(|p| {
                            ed.rig_data
                                .borrow()
                                .pose
                                .bones
                                .iter()
                                .find(|x| &x.id == p)
                                .map(|x| x.angle)
                        })
                        .unwrap_or(0.0);
                    let rot = ((ev.angle - parent_angle + 540.0).rem_euclid(360.0) - 180.0).round();
                    let cmd = set(
                        s2::key(RIG, &rig.id, &format!("bone.{}.rot", b.id)),
                        Value::Float(rot),
                    );
                    if ev.phase == StrokePhase::Begin {
                        ed.sel_bone = Some(b.id.clone());
                        select(act.ui, ed.bones, key_of(&["bone", &rig.id, &b.id]));
                        let mut g = act.cmd.gesture(&forge_ui::trf!("Rotate {name}", name = b.name));
                        g.update(cmd);
                        ed.bone_drag = Some(g);
                    } else if let Some(g) = ed.bone_drag.as_mut() {
                        g.update(cmd);
                    }
                    // Show the drag at once (the mirror follows).
                    if let Some(bb) = ed
                        .doc
                        .rigs
                        .get_mut(&rig.id)
                        .and_then(|r| r.bones.get_mut(&b.id))
                    {
                        bb.rot = rot;
                    }
                    ed.show_views(act.ui, act.services);
                }
            }
        });

        // ---- sync -------------------------------------------------------------------------
        let e = ed;
        pb.sync(tiles, move |s| {
            let rev = watch(s.mirror, PREFIX);
            let mut ed = e.borrow_mut();
            if rev != ed.seen {
                ed.seen = rev;
                ed.refresh(s.ui, s.mirror, s.services);
            }
            Ok(())
        });
        Ok(())
    });
}

fn sel_anim(ed: &Ed) -> Option<(String, s2::Anim)> {
    let sh = ed.sel_sheet.clone()?;
    let a = ed
        .doc
        .sheets
        .get(&sh)?
        .anims
        .get(ed.sel_anim.as_ref()?)?
        .clone();
    Some((sh, a))
}

/// The commands a paint stroke over `cells` writes (for tests and automation sessions: the same
/// path the canvas takes).
pub fn stroke_commands(
    doc: &Doc2d,
    map: &str,
    layer: u32,
    cells: &[(i64, i64)],
    cell: &MapCell,
) -> Vec<EditorCommand> {
    let Some(l) = doc.tilemaps.get(map).and_then(|t| t.layers.get(&layer)) else {
        return Vec::new();
    };
    let mut st = Stroke::default();
    for (x, y) in cells {
        st.paint(l, *x, *y, cell.clone());
    }
    st.commands(map, layer)
}
