//! `C-ui-asset-index`: the asset reference picker's index is the real asset database —
//! [`CatalogIndex`] over the editor's catalogue sees an import and a rename an automation session
//! makes through the bus (the shell follows the project), with no copy to go stale.
//!
//! Positive control (W2): `positive_control_a_snapshot_index_goes_stale` — the labelled
//! in-memory stand-in (`MemoryAssetIndex`) filled before the edits fails the same check.

use std::cell::RefCell;
use std::rc::Rc;

use forge_cmd::Issuer;
use forge_editor::assets::{AssetCatalog, ServerCatalog, import_command, rename_command};
use forge_editor::client::BusClient;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::testing::Rig;
use forge_panels_assets::CatalogIndex;
use forge_ui::widgets::{AssetEntry, AssetIndex, MemoryAssetIndex};

/// A shell whose asset catalogue holds two PNGs, `tex/a.png` imported by an automation session.
fn setup() -> (Rig, Rc<RefCell<dyn AssetCatalog>>) {
    let png = forge_asset::fixture::checker_png(8, [0, 0, 0, 255], [255; 4]).to_vec();
    let files = vec![
        ("tex/a.png".to_string(), png.clone()),
        ("tex/b.png".to_string(), png),
    ];
    let cat: Rc<RefCell<dyn AssetCatalog>> = Rc::new(RefCell::new(
        ServerCatalog::over_memory_files(&files).unwrap_or_else(|e| panic!("{e}")),
    ));
    let mut cfg = assemble(
        builtin_preset("3d").unwrap_or_else(|e| panic!("{e}")),
        &[],
        &[],
        None,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    cfg.services.assets = Some(cat.clone());
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    auto(&mut rig, import_command("tex/a.png"));
    (rig, cat)
}

fn auto(rig: &mut Rig, cmd: forge_cmd::EditorCommand) {
    let mut a = rig.connect(Issuer::Automation {
        session: "s".into(),
        tool: "t".into(),
    });
    a.apply(cmd, None);
    let _ = a.pump();
    rig.turn();
}

fn check(index: &dyn AssetIndex, rig: &mut Rig) -> Result<(), String> {
    let texture = |p: &str| {
        Some(AssetEntry {
            path: p.into(),
            kind: "texture".into(),
        })
    };
    if index.get("tex/a.png") != texture("tex/a.png") {
        return Err(format!(
            "the imported asset is missing: {:?}",
            index.get("tex/a.png")
        ));
    }
    auto(rig, import_command("tex/b.png"));
    auto(rig, rename_command("tex/a.png", "tex/wall.png"));
    let textures: Vec<String> = index
        .of_kind("Texture")
        .into_iter()
        .map(|e| e.path)
        .collect();
    if textures != vec!["tex/b.png".to_string(), "tex/wall.png".to_string()] {
        return Err(format!("the index is stale: {textures:?}"));
    }
    if index.get("tex/a.png").is_some() {
        return Err("the index is stale: the renamed-away path is still offered".into());
    }
    Ok(())
}

#[test]
fn the_picker_index_is_the_asset_database() {
    let (mut rig, cat) = setup();
    let index = CatalogIndex::new(cat);
    check(&index, &mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_snapshot_index_goes_stale() {
    let (mut rig, cat) = setup();
    let snapshot = MemoryAssetIndex {
        entries: cat
            .borrow()
            .rows()
            .into_iter()
            .map(|r| AssetEntry {
                path: r.path,
                kind: r.kind,
            })
            .collect(),
    };
    let e = check(&snapshot, &mut rig).err().unwrap_or_default();
    assert!(e.contains("stale"), "a snapshot index passed: {e:?}");
}
