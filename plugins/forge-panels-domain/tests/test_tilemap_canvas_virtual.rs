//! `test_tilemap_canvas_virtual` (Ch.21 §21.21 "2D editors", D-5, gate row
//! `C-tilemap-canvas-virtual`; DoD M2-66): the tile-map canvas visits only the cells in
//! view. Painting a 512 × 512 map (262,144 painted cells) costs what a screenful costs: the cells a
//! paint visits are at most ⌈width ÷ cell⌉ × ⌈height ÷ cell⌉, however large the map. (What a
//! stroke step costs on such a map is `test_tile_stroke_bounded`.)
//!
//! Positive control (W2): `positive_control_canvas_visiting_every_cell_fails` — the
//! canvas walks every painted cell of the layer; the bound fails.

mod common;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::domain::scene2d::{CHUNK, Cell, Chunk, chunk_key};
use forge_editor::services::PanelFaults;
use forge_panels_domain::widgets::TileCanvas;

const P: &str = "forge.editors_2d";
/// Chunks per side: 32 × 16 = 512 cells.
const SIDE: i64 = 32;

fn run(faults: PanelFaults) -> Result<(usize, usize), String> {
    let mut rig = common::rig_with(&[P], faults, |_| {});
    let mut script = rig.connect(Issuer::Script {
        path: "big_map.fscript".into(),
    });
    let mut chunk = Chunk::default();
    for (i, c) in chunk.cells.iter_mut().enumerate() {
        *c = Cell::Tile((i % 48) as u32);
    }
    let text = chunk.encode();
    let mut cmds = vec![
        EditorCommand::SetSetting {
            key: "d2.tileset.ts.count".into(),
            value: Some(Value::Int(48)),
        },
        EditorCommand::SetSetting {
            key: "d2.tilemap.big.name".into(),
            value: Some(Value::Text("Big".into())),
        },
        EditorCommand::SetSetting {
            key: "d2.tilemap.big.tileset".into(),
            value: Some(Value::Text("ts".into())),
        },
        EditorCommand::SetSetting {
            key: "d2.tilemap.big.layer.0.name".into(),
            value: Some(Value::Text("Ground".into())),
        },
    ];
    for cy in 0..SIDE {
        for cx in 0..SIDE {
            cmds.push(EditorCommand::SetSetting {
                key: chunk_key("big", 0, (cx, cy)),
                value: Some(Value::Text(text.clone())),
            });
        }
    }
    for c in cmds {
        script.apply(c, None);
    }
    let _ = script.pump();
    rig.settle();
    let maps = common::part(&rig, P, &["tiles", "body", "side", "maps"]);
    let layer = common::rows_with(&mut rig, maps, "Layer 0");
    let layer = *layer.first().ok_or("the big map's layer is not listed")?;
    common::select_row(&mut rig, maps, layer);
    let canvas = common::part(&rig, P, &["tiles", "body", "main", "canvas"]);
    rig.h.ui.damage_all();
    rig.settle();
    let r = rig.h.ui.rect(canvas).ok_or("the canvas has no rect")?;
    let bound = ((r.w / 20.0).ceil() * (r.h / 20.0).ceil()) as usize;
    let visited = rig
        .h
        .ui
        .widget::<TileCanvas>(canvas)
        .map_or(0, TileCanvas::painted_cells);
    let total = (SIDE * CHUNK * SIDE * CHUNK) as usize;
    if visited == 0 {
        return Err("the canvas painted nothing".into());
    }
    if visited > bound {
        return Err(format!(
            "{visited} cells visited for a {:.0} × {:.0} canvas over {total} painted cells (bound {bound})",
            r.w, r.h
        ));
    }
    Ok((visited, total))
}

#[test]
fn test_tilemap_canvas_virtual() {
    let (visited, total) = run(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    println!("test_tilemap_canvas_virtual: {visited} cells visited over a {total}-cell map");
}

#[test]
fn positive_control_canvas_visiting_every_cell_fails() {
    let e = run(PanelFaults {
        tilemap_paint_every_cell: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("cells visited"),
        "a canvas visiting every cell passed: {e:?}"
    );
}
