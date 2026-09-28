//! The 2D editors (`forge.editors_2d`, DoD M2-66) through the running shell: tile sets
//! and autotile rules, painting a tile map, sprite-sheet slicing and frame animations, and
//! the `Skeleton2D` rig — every edit a command, every command undoable.

mod common;

use std::rc::Rc;

use common::{history_len, last_notice, part, press, raise, rows_with, select_row, setting, undo};
use forge_cmd::Value;
use forge_editor::domain::scene2d::{Forge2dScene, Scene2d, blob47};
use forge_panels_domain::widgets::{BoneDrag, FramePicked, PaintCells, StrokePhase, TilePicked};
use forge_ui::widgets::{DropTarget, IntegerCommitted, NumericCommitted, RowsDropped};
use forge_ui::{KeyCode, Modifiers, WidgetId};

const P: &str = "forge.editors_2d";

fn w(rig: &forge_editor::testing::Rig, path: &[&str]) -> WidgetId {
    part(rig, P, path)
}

fn text(v: Option<Value>) -> String {
    match v {
        Some(Value::Text(t)) => t,
        other => format!("{other:?}"),
    }
}

/// A real PNG file of `w x h` (RGBA, a gradient).
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("png header");
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 256) as u8, 40, 90, 255])
            .collect();
        wr.write_image_data(&px).expect("png data");
    }
    out
}

fn a11y_value(rig: &mut forge_editor::testing::Rig, id: WidgetId) -> String {
    rig.h
        .node(id)
        .and_then(|n| n.value().map(str::to_string))
        .unwrap_or_default()
}

#[test]
fn tile_sets_rules_and_a_painted_terrain_stroke_are_commands_and_undo() {
    let mut rig = common::rig(&[P]);
    let start = rig.state_hash();
    {
        let b = w(&rig, &["tiles", "bar", "new_tileset"]);
        press(&mut rig, b);
    }
    assert_eq!(
        setting(&rig, "d2.tileset.tile_set.name"),
        Some(Value::Text("Tile set".into()))
    );
    assert_eq!(
        setting(&rig, "d2.tileset.tile_set.count"),
        Some(Value::Int(48))
    );
    // Select the Ground terrain (the brush) and fill its rules from the blob template.
    let tiles = w(&rig, &["tiles", "body", "side", "tilesets"]);
    let ground = rows_with(&mut rig, tiles, "Ground");
    assert_eq!(ground.len(), 1, "the new tile set lists its terrain");
    select_row(&mut rig, tiles, ground[0]);
    {
        let b = w(&rig, &["tiles", "bar", "blob_template"]);
        press(&mut rig, b);
    }
    let rules = rig
        .shell
        .mirror()
        .settings_under("d2.tileset.tile_set.terrain.ground.rule.")
        .count();
    assert_eq!(rules, 47, "all 47 canonical masks get a tile");
    // A map over it; paint on its layer.
    {
        let b = w(&rig, &["tiles", "bar", "new_map"]);
        press(&mut rig, b);
    }
    assert_eq!(
        setting(&rig, "d2.tilemap.tile_map.tileset"),
        Some(Value::Text("tile_set".into()))
    );
    let maps = w(&rig, &["tiles", "body", "side", "maps"]);
    let layer = rows_with(&mut rig, maps, "Layer 0");
    select_row(&mut rig, maps, layer[0]);
    let after_setup = rig.state_hash();
    let before = history_len(&rig);
    let canvas = w(&rig, &["tiles", "body", "main", "canvas"]);
    let stroke = [
        (vec![(0, 0)], StrokePhase::Begin),
        (vec![(1, 0), (2, 0)], StrokePhase::Move),
        (vec![(2, 1)], StrokePhase::Move),
        (vec![], StrokePhase::End),
    ];
    for (cells, phase) in stroke {
        raise(
            &mut rig,
            canvas,
            PaintCells {
                cells,
                erase: false,
                phase,
            },
        );
    }
    assert_eq!(history_len(&rig), before + 1, "a stroke is one undo entry");
    let entry = rig
        .shell
        .mirror()
        .history()
        .last()
        .map(|h| h.label.clone())
        .unwrap_or_default();
    assert_eq!(entry, "Paint tiles");
    let chunk = text(setting(&rig, "d2.tilemap.tile_map.layer.0.chunk.c0_0"));
    assert_eq!(chunk.matches("aground").count(), 4, "{chunk}");
    // The canvas shows cell (1, 0) with its solved tile: W and E neighbours.
    rig.h.ui.set_focus(Some(canvas), true);
    rig.turn();
    rig.chord(KeyCode::Right, Modifiers::NONE);
    rig.settle();
    let want = blob47()
        .iter()
        .position(|m| *m == (forge_editor::domain::scene2d::E | forge_editor::domain::scene2d::W))
        .unwrap_or(99);
    let value = a11y_value(&mut rig, canvas);
    assert!(
        value.contains(&format!("Cell 1, 0: terrain ground, tile {want}")),
        "{value}"
    );
    // Space paints at the cursor (the keyboard path), with a tile brush from the palette.
    {
        let b = w(&rig, &["tiles", "body", "main", "palette"]);
        raise(&mut rig, b, TilePicked { tile: 7 });
    }
    rig.h.ui.set_focus(Some(canvas), true);
    rig.chord(KeyCode::Down, Modifiers::NONE);
    rig.chord(KeyCode::Space, Modifiers::NONE);
    rig.settle();
    let chunk = text(setting(&rig, "d2.tilemap.tile_map.layer.0.chunk.c0_0"));
    assert!(chunk.contains("t7"), "{chunk}");
    // Erase with the right button stroke: cell (0, 0).
    raise(
        &mut rig,
        canvas,
        PaintCells {
            cells: vec![(0, 0)],
            erase: true,
            phase: StrokePhase::Begin,
        },
    );
    raise(
        &mut rig,
        canvas,
        PaintCells {
            cells: vec![],
            erase: true,
            phase: StrokePhase::End,
        },
    );
    let chunk = text(setting(&rig, "d2.tilemap.tile_map.layer.0.chunk.c0_0"));
    assert_eq!(chunk.matches("aground").count(), 3, "{chunk}");
    // Undo the erase, the tile, the stroke: back to the set-up state.
    undo(&mut rig);
    undo(&mut rig);
    undo(&mut rig);
    assert_eq!(rig.state_hash(), after_setup);
    assert_eq!(
        setting(&rig, "d2.tilemap.tile_map.layer.0.chunk.c0_0"),
        None
    );
    // And every set-up step undoes too.
    for _ in 0..3 {
        undo(&mut rig);
    }
    assert_eq!(rig.state_hash(), start);
    assert!(rig.mirror_matches());
}

#[test]
fn assigning_a_rule_and_refusing_to_paint_without_a_layer() {
    let mut rig = common::rig(&[P]);
    {
        let b = w(&rig, &["tiles", "bar", "new_tileset"]);
        press(&mut rig, b);
    }
    // No map selected: painting is refused with the reason.
    let canvas = w(&rig, &["tiles", "body", "main", "canvas"]);
    let before = history_len(&rig);
    raise(
        &mut rig,
        canvas,
        PaintCells {
            cells: vec![(0, 0)],
            erase: false,
            phase: StrokePhase::Single,
        },
    );
    assert_eq!(history_len(&rig), before);
    let (title, why) = last_notice(&rig).unwrap_or_default();
    assert!(
        title.contains("Nothing to paint") && why.contains("layer"),
        "{title}: {why}"
    );
    // Assign mode: a palette click sets the selected rule's tile.
    let tiles = w(&rig, &["tiles", "body", "side", "tilesets"]);
    let ground = rows_with(&mut rig, tiles, "Ground");
    select_row(&mut rig, tiles, ground[0]);
    let rules = w(&rig, &["tiles", "body", "side", "rules"]);
    let iso = rows_with(&mut rig, rules, "isolated");
    assert_eq!(iso.len(), 1, "47 rule rows, one isolated");
    select_row(&mut rig, rules, iso[0]);
    let mode = w(&rig, &["tiles", "bar", "mode"]);
    rig.h.ui.set_focus(Some(mode), true);
    rig.chord(KeyCode::Down, Modifiers::NONE);
    rig.settle();
    {
        let b = w(&rig, &["tiles", "body", "main", "palette"]);
        raise(&mut rig, b, TilePicked { tile: 5 });
    }
    assert_eq!(
        setting(&rig, "d2.tileset.tile_set.terrain.ground.rule.0"),
        Some(Value::Int(5))
    );
}

#[test]
fn sprite_sheets_slice_and_animations_pick_frames() {
    // The project holds a real 128 x 64 PNG; forge-2d reads its size from the file header.
    let png = png_bytes(128, 64);
    let scene: Rc<dyn Scene2d> = Rc::new(Forge2dScene::with_source(Rc::new(move |p: &str| {
        (p == "sprites/sheet.png").then(|| png.clone())
    })));
    assert_eq!(scene.backend().name, "forge-2d");
    assert!(
        !scene.backend().in_memory,
        "the real 2D pipeline, not a stand-in"
    );
    let mut rig = common::rig_with(&[P], Default::default(), move |sv| sv.scene2d = scene);
    // Open the Sprite sheet tab from the keyboard.
    let tabs = w(&rig, &["tabs"]);
    rig.h.ui.set_focus(Some(tabs), true);
    rig.chord(KeyCode::Right, Modifiers::NONE);
    rig.settle();
    assert!(
        !rig.h.ui.is_hidden(w(&rig, &["sheet"])),
        "the tab shows its page"
    );
    {
        let b = w(&rig, &["sheet", "bar", "new_sheet"]);
        press(&mut rig, b);
    }
    assert_eq!(
        setting(&rig, "d2.sheet.sheet.image_w"),
        Some(Value::Int(128)),
        "the size comes from the backend"
    );
    let sheets = w(&rig, &["sheet", "body", "side", "sheets"]);
    let row = rows_with(&mut rig, sheets, "sheet");
    select_row(&mut rig, sheets, row[0]);
    let cell_w = w(&rig, &["sheet", "body", "side", "slice", "cell_w"]);
    raise(
        &mut rig,
        cell_w,
        IntegerCommitted {
            field: cell_w,
            value: 16,
        },
    );
    assert_eq!(setting(&rig, "d2.sheet.sheet.cell_w"), Some(Value::Int(16)));
    let preview = w(&rig, &["sheet", "body", "preview"]);
    let label = rig
        .h
        .node(preview)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default();
    assert!(
        label.contains("16 frames"),
        "8 × 2 cells of 16 × 32: {label}"
    );
    {
        let b = w(&rig, &["sheet", "bar", "new_anim"]);
        press(&mut rig, b);
    }
    let anims = w(&rig, &["sheet", "body", "side", "anims"]);
    let a = rows_with(&mut rig, anims, "Animation 1");
    select_row(&mut rig, anims, a[0]);
    for f in [3, 5, 9] {
        raise(&mut rig, preview, FramePicked { frame: f });
    }
    assert_eq!(
        setting(&rig, "d2.sheet.sheet.anim.anim.frames"),
        Some(Value::Text("3,5,9".into()))
    );
    {
        let b = w(&rig, &["sheet", "bar", "drop_frame"]);
        press(&mut rig, b);
    }
    assert_eq!(
        setting(&rig, "d2.sheet.sheet.anim.anim.frames"),
        Some(Value::Text("3,5".into()))
    );
    let fps = w(&rig, &["sheet", "body", "side", "anim_fields", "fps"]);
    raise(
        &mut rig,
        fps,
        NumericCommitted {
            field: fps,
            value: 24.0,
        },
    );
    assert_eq!(
        setting(&rig, "d2.sheet.sheet.anim.anim.fps"),
        Some(Value::Float(24.0))
    );
    let n = history_len(&rig);
    undo(&mut rig);
    assert_eq!(
        setting(&rig, "d2.sheet.sheet.anim.anim.fps"),
        Some(Value::Float(12.0))
    );
    assert_eq!(history_len(&rig), n, "undo keeps the entry (redoable)");
}

#[test]
fn rig_bones_reparent_refuses_loops_and_a_tip_drag_is_one_gesture() {
    let mut rig = common::rig(&[P]);
    {
        let b = w(&rig, &["rig", "bar", "new_rig"]);
        press(&mut rig, b);
    }
    let rigs = w(&rig, &["rig", "body", "side", "rigs"]);
    let r = rows_with(&mut rig, rigs, "Rig");
    select_row(&mut rig, rigs, r[0]);
    let bones = w(&rig, &["rig", "body", "side", "bones"]);
    let root = rows_with(&mut rig, bones, "Root");
    select_row(&mut rig, bones, root[0]);
    {
        let b = w(&rig, &["rig", "bar", "add_bone"]);
        press(&mut rig, b);
    }
    assert_eq!(
        setting(&rig, "d2.rig.rig.bone.bone.parent"),
        Some(Value::Text("root".into()))
    );
    assert_eq!(
        setting(&rig, "d2.rig.rig.bone.bone.x"),
        Some(Value::Float(48.0)),
        "at the parent's tip"
    );
    // Dragging the root under its child would make a loop: refused, named.
    let child = rows_with(&mut rig, bones, "Bone");
    let before = history_len(&rig);
    raise(
        &mut rig,
        bones,
        RowsDropped {
            view: bones,
            keys: vec![root[0]],
            target: DropTarget {
                parent: Some(child[0]),
                index: 0,
            },
        },
    );
    assert_eq!(history_len(&rig), before);
    let (_, why) = last_notice(&rig).unwrap_or_default();
    assert!(why.contains("own ancestor"), "{why}");
    // A tip drag: begin, three moves, end = one entry, the last angle kept.
    let view = w(&rig, &["rig", "body", "view"]);
    let before = history_len(&rig);
    for (angle, phase) in [
        (10.0, StrokePhase::Begin),
        (20.0, StrokePhase::Move),
        (30.0, StrokePhase::Move),
        (45.0, StrokePhase::Move),
        (0.0, StrokePhase::End),
    ] {
        raise(
            &mut rig,
            view,
            BoneDrag {
                bone: "bone".into(),
                angle,
                phase,
            },
        );
    }
    assert_eq!(history_len(&rig), before + 1, "one gesture, one undo entry");
    assert_eq!(
        setting(&rig, "d2.rig.rig.bone.bone.rot"),
        Some(Value::Float(45.0))
    );
    // The exact field.
    select_row(&mut rig, bones, child[0]);
    let x = w(&rig, &["rig", "body", "side", "bone_fields", "x"]);
    raise(
        &mut rig,
        x,
        NumericCommitted {
            field: x,
            value: 12.5,
        },
    );
    assert_eq!(
        setting(&rig, "d2.rig.rig.bone.bone.x"),
        Some(Value::Float(12.5))
    );
    undo(&mut rig);
    undo(&mut rig);
    assert_eq!(setting(&rig, "d2.rig.rig.bone.bone.rot"), None);
    // Delete removes the bone (and its children).
    raise(
        &mut rig,
        bones,
        forge_ui::widgets::RowsDeleteRequested {
            view: bones,
            keys: vec![child[0]],
        },
    );
    assert_eq!(setting(&rig, "d2.rig.rig.bone.bone.name"), None);
    assert!(setting(&rig, "d2.rig.rig.bone.root.name").is_some());
}

#[test]
fn the_2d_preset_opens_the_2d_editors_and_every_preset_can() {
    for preset in forge_editor::presets::BUILTIN_PRESETS.iter().copied() {
        let cfg = common::config(preset);
        let mut rig = forge_editor::testing::Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
        rig.show_panels(&[P])
            .unwrap_or_else(|e| panic!("{preset}: {e}"));
        assert!(rig.panel_frame(P).is_some(), "{preset}");
    }
}

/// C-editors-2d-backend: the editors run on the real 2D pipeline by default — the rig panel's
/// pose is `forge_2d::skeleton::Skeleton2d`'s forward kinematics and the footer names
/// forge-2d, not a stand-in.
#[test]
fn the_2d_editors_run_on_forge_2d_by_default() {
    let mut rig = common::rig(&[P]);
    let info = rig.shell.handles().services_rc().scene2d.backend();
    assert_eq!(info.name, "forge-2d");
    assert!(!info.in_memory);
    let footer = w(&rig, &["backend"]);
    let label = rig
        .h
        .node(footer)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default();
    assert!(label.contains("forge-2d"), "{label}");
    // The backend's pose is forge-2d's FK: a bone turned 90 degrees with a child along it.
    use forge_editor::domain::scene2d::{Bone, Rig2d};
    let mut r = Rig2d::default();
    r.bones.insert(
        "a".into(),
        Bone {
            id: "a".into(),
            rot: 90.0,
            len: 10.0,
            ..Bone::default()
        },
    );
    r.bones.insert(
        "b".into(),
        Bone {
            id: "b".into(),
            parent: Some("a".into()),
            x: 10.0,
            len: 5.0,
            ..Bone::default()
        },
    );
    let pose = rig.shell.handles().services_rc().scene2d.pose(&r);
    let b = pose.bones.iter().find(|x| x.id == "b").cloned();
    let tip = b.map(|b| b.tip).unwrap_or_default();
    assert!(
        (tip.0).abs() < 1e-9 && (tip.1 - 15.0).abs() < 1e-9,
        "{tip:?}"
    );
}

/// Open the Sprite sheet tab and press New sheet with the image field empty (the editor
/// uses `sprites/sheet.png`); the new sheet's image size as the editor recorded it.
fn new_sheet_size(rig: &mut forge_editor::testing::Rig) -> (Option<Value>, Option<Value>) {
    let tabs = w(rig, &["tabs"]);
    rig.h.ui.set_focus(Some(tabs), true);
    rig.chord(KeyCode::Right, Modifiers::NONE);
    rig.settle();
    let b = w(rig, &["sheet", "bar", "new_sheet"]);
    press(rig, b);
    (
        setting(rig, "d2.sheet.sheet.image_w"),
        setting(rig, "d2.sheet.sheet.image_h"),
    )
}

/// The editor as the binary wires it (`EditorServices::set_assets` with the project's asset
/// catalogue) reads image sizes from the project's files: a PNG's IHDR, an Aseprite file's
/// header (the sheet the importer lays its frames out on). Control: the same editor without
/// the catalogue attached has no file to read and falls back to 256 x 256, so the 96 x 48
/// really came from the file.
#[test]
fn the_shipped_wiring_reads_image_sizes_from_the_project_files() {
    use forge_editor::assets::{AssetCatalog, ServerCatalog};
    use std::cell::RefCell;
    let ase = forge_2d::aseprite::write(&forge_2d::aseprite::AseWrite {
        w: 8,
        h: 6,
        layers: vec![("a".into(), true, 255)],
        frames: vec![(100, Vec::new()); 3],
        tags: Vec::new(),
        compress: false,
    });
    let files = vec![
        ("sprites/sheet.png".to_string(), png_bytes(96, 48)),
        ("art/hero.aseprite".to_string(), ase),
    ];
    let catalog = ServerCatalog::over_memory_files(&files).unwrap_or_else(|e| panic!("{e}"));
    let catalog: Rc<RefCell<dyn AssetCatalog>> = Rc::new(RefCell::new(catalog));
    let mut rig = common::rig_with(&[P], Default::default(), move |sv| sv.set_assets(catalog));
    assert_eq!(
        new_sheet_size(&mut rig),
        (Some(Value::Int(96)), Some(Value::Int(48))),
        "the PNG header, read through the asset catalogue"
    );
    let scene = rig.shell.handles().services_rc().scene2d.clone();
    assert_eq!(scene.backend().name, "forge-2d");
    assert_eq!(
        scene.image_size("art/hero.aseprite"),
        Some((16, 12)),
        "three 8 x 6 frames on a 2 x 2 sheet"
    );
    assert_eq!(scene.image_size("art/missing.png"), None);

    let mut bare = common::rig(&[P]);
    assert_eq!(
        new_sheet_size(&mut bare),
        (Some(Value::Int(256)), Some(Value::Int(256))),
        "control: no catalogue, no file to read"
    );
}

/// The tiles a painted terrain shows in the editor, cell by cell (the canvas asks
/// `Scene2d::autotile` with the cell's mask, as `widgets::TileCanvas` does).
fn shown_tiles(scene: &dyn Scene2d, cells: &[(i64, i64)]) -> Vec<Option<u32>> {
    use forge_editor::domain::scene2d::{Cell, Layer, Stroke, Terrain, chunk_of};
    let mut terrain = Terrain {
        id: "ground".into(),
        ..Terrain::default()
    };
    for (i, m) in blob47().into_iter().enumerate() {
        terrain.rules.insert(m, i as u32);
    }
    let mut layer = Layer::default();
    for (x, y) in cells {
        let (c, i) = chunk_of(*x, *y);
        layer.chunks.entry(c).or_default().cells[i] = Cell::Terrain("ground".into());
    }
    let none = Stroke::default();
    cells
        .iter()
        .map(|(x, y)| scene.autotile(&terrain, layer.mask(&none, *x, *y, "ground")))
        .collect()
}

/// The tiles the game draws for the same cells: a `forge_2d::tilemap::Tilemap`.
fn game_tiles(cells: &[(i64, i64)]) -> Vec<Option<u32>> {
    use forge_2d::tilemap::{Cell, Tilemap, Tileset};
    let mut rules = std::collections::BTreeMap::new();
    for (i, m) in forge_2d::tilemap::blob47().into_iter().enumerate() {
        rules.insert(m, i as u32);
    }
    let ts = Tileset {
        texture: forge_2d::sprite::TextureId(0),
        texture_size: (128, 96),
        origin_px: (0, 0),
        tile_px: (16, 16),
        columns: 8,
        count: 48,
        terrains: vec![rules],
        solid: Default::default(),
    };
    let mut map = Tilemap::new(
        forge_2d::FramePos2::new(forge_2d::FrameId(0), forge_2d::DVec2::ZERO),
        forge_2d::DVec2::new(1.0, 1.0),
        ts,
    );
    let l = map.add_layer("ground", 0, false);
    for (x, y) in cells {
        map.set(l, *x as i32, *y as i32, Cell::Terrain(0))
            .expect("set");
    }
    cells
        .iter()
        .map(|(x, y)| map.resolve(l, *x as i32, *y as i32).expect("resolve"))
        .collect()
}

/// A scattered, chunk-straddling pattern (every blob shape shows up).
fn pattern() -> Vec<(i64, i64)> {
    let s = forge_2d::math::Stream::new(0x2D_ED17);
    let mut v: Vec<(i64, i64)> = (0..600u64)
        .map(|i| {
            (
                (s.bits(2 * i) % 40) as i64 - 20,
                (s.bits(2 * i + 1) % 40) as i64 - 20,
            )
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// C-editors-2d-backend: what the tile editor shows is exactly what the game draws — the
/// editor's backend is forge-2d's own solve, over a pattern crossing chunk borders.
#[test]
fn the_editor_shows_the_tiles_the_game_draws() {
    let cells = pattern();
    let scene = Forge2dScene::new();
    let shown = shown_tiles(&scene, &cells);
    assert_eq!(shown, game_tiles(&cells));
    let distinct: std::collections::BTreeSet<Option<u32>> = shown.into_iter().collect();
    assert!(
        distinct.len() > 20,
        "the pattern exercises many blob shapes: {}",
        distinct.len()
    );
}

/// W2 control: a backend with its own rule (corners ignored — the stand-in's old edges-only
/// shortcut) shows tiles the game does not draw, and the comparison catches it.
#[test]
fn positive_control_a_backend_with_its_own_rule_is_caught() {
    struct EdgesOnly(Forge2dScene);
    impl Scene2d for EdgesOnly {
        fn backend(&self) -> forge_editor::domain::BackendInfo {
            self.0.backend()
        }
        fn autotile(&self, t: &forge_editor::domain::scene2d::Terrain, mask: u8) -> Option<u32> {
            use forge_editor::domain::scene2d::{E, N, S, W};
            self.0.autotile(t, mask & (N | E | S | W))
        }
        fn slice(
            &self,
            s: &forge_editor::domain::scene2d::SheetSlice,
        ) -> Vec<forge_editor::domain::scene2d::FrameRect> {
            self.0.slice(s)
        }
        fn pose(
            &self,
            r: &forge_editor::domain::scene2d::Rig2d,
        ) -> forge_editor::domain::scene2d::Pose {
            self.0.pose(r)
        }
        fn image_size(&self, i: &str) -> Option<(i64, i64)> {
            self.0.image_size(i)
        }
    }
    let cells = pattern();
    let wrong = EdgesOnly(Forge2dScene::new());
    assert_ne!(shown_tiles(&wrong, &cells), game_tiles(&cells));
}
