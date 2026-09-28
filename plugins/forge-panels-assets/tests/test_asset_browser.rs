//! The asset browser through the running shell over the real asset database (DoD M2-38):
//! OS file drops import through commands, an automation session's import shows up, thumbnails
//! arrive asynchronously and wake the loop only when ready, rename / move / remove are commands
//! (undoable, the database follows the project), grid and list views, folder navigation, search,
//! lock indicators, and zero idle once everything is loaded.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use forge_cmd::Issuer;
use forge_editor::EditorError;
use forge_editor::assets::{AssetCatalog, AssetRow, ServerCatalog, ThumbDone, import_command};
use forge_editor::client::BusClient;
use forge_editor::mirror::ProjectMirror;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_panels_assets::PanelsAssets;
use forge_panels_core::PanelsCore;
use forge_plugin::SourcePlugin;
use forge_ui::widgets::{
    DropTarget, FilesDropped, GridMode, GridUp, RowActivated, RowRenamed, RowsDeleteRequested,
    RowsDropped, SearchChanged, VirtualGrid,
};
use forge_ui::{Key, WidgetId};

const P: &str = "forge.assets";

fn rig_over(catalog: Rc<RefCell<dyn AssetCatalog>>) -> Rig {
    let core = PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let assets = PanelsAssets::new().unwrap_or_else(|e| panic!("{e}"));
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_assets::panel_ids());
    let stand_in = StandInPanels::without(&real).unwrap_or_else(|e| panic!("{e}"));
    let all: Vec<&dyn SourcePlugin> = vec![&core, &assets, &stand_in];
    let mut cfg = assemble(
        builtin_preset("3d").unwrap_or_else(|e| panic!("{e}")),
        &all,
        &[],
        None,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    cfg.services.assets = Some(catalog);
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&[P]).unwrap_or_else(|e| panic!("{e}"));
    rig
}

fn part(rig: &Rig, path: &[&str]) -> WidgetId {
    let mut id = rig.panel_frame(P).unwrap_or_else(|| panic!("open"));
    for k in path {
        id = id.child(&Key::Str((*k).into()));
    }
    assert!(rig.h.ui.contains(id), "no widget at {path:?}");
    id
}

fn tiles(rig: &mut Rig) -> Vec<(u64, String)> {
    let g = part(rig, &["grid"]);
    VirtualGrid::edit(&mut rig.h.ui, g, |v| {
        (0..v.len())
            .filter_map(|i| {
                v.key_at(i)
                    .and_then(|k| v.tile(k).map(|t| (k, t.label.clone())))
            })
            .collect()
    })
    .unwrap_or_default()
}

fn key_of(rig: &mut Rig, label: &str) -> u64 {
    tiles(rig)
        .into_iter()
        .find(|(_, l)| l == label)
        .map(|(k, _)| k)
        .unwrap_or_else(|| panic!("no tile {label:?} in {:?}", tiles(rig)))
}

fn server_rig() -> (Rig, Rc<RefCell<ServerCatalog>>) {
    let files: Vec<(String, Vec<u8>)> =
        forge_asset::fixture::scene(&forge_asset::fixture::SceneSpec::default())
            .into_iter()
            .map(|(n, b)| (format!("models/{n}"), b.to_vec()))
            .collect();
    let catalog = Rc::new(RefCell::new(
        ServerCatalog::over_memory_files(&files).unwrap_or_else(|e| panic!("{e}")),
    ));
    let dynamic: Rc<RefCell<dyn AssetCatalog>> = catalog.clone();
    (rig_over(dynamic), catalog)
}

/// Turn until `f` holds (thumbnails render on worker threads), at most ~10 s.
fn turn_until(rig: &mut Rig, mut f: impl FnMut(&mut Rig) -> bool) -> bool {
    for _ in 0..1000 {
        rig.turn();
        if f(rig) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn temp_png(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("forge-asset-browser-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    let p = d.join(name);
    let bytes = forge_asset::fixture::checker_png(16, [255, 0, 0, 255], [0, 0, 255, 255]);
    std::fs::write(&p, &bytes).unwrap_or_else(|e| panic!("{e}"));
    p
}

#[test]
fn drops_imports_renames_moves_and_removes_are_commands() {
    let (mut rig, catalog) = server_rig();
    assert!(tiles(&mut rig).is_empty(), "nothing imported yet");
    let start = rig.state_hash();

    // Drop an OS file: copied into the folder shown, imported by one command.
    let png = temp_png("bricks.png");
    let g = part(&rig, &["grid"]);
    rig.h.ui.raise(
        g,
        FilesDropped {
            view: g,
            files: vec![png],
            folder: None,
        },
    );
    rig.turn();
    rig.turn();
    let entry = rig
        .shell
        .mirror()
        .history()
        .last()
        .map(|h| (h.label.clone(), h.issuer_tag()));
    assert_eq!(
        entry,
        Some(("Import 1 asset(s)".into(), "human:tester".into()))
    );
    let bricks = key_of(&mut rig, "bricks.png");
    assert!(
        catalog
            .borrow()
            .rows()
            .iter()
            .any(|r| r.path == "bricks.png" && r.kind == "texture")
    );

    // An automation session imports the glTF scene: its folder appears here by the same path.
    let mut auto = rig.connect(Issuer::Automation {
        session: "s1".into(),
        tool: "import".into(),
    });
    auto.apply(import_command("models/scene.gltf"), None);
    let _ = auto.pump();
    rig.turn();
    rig.turn();
    let models = key_of(&mut rig, "models");
    assert!(
        VirtualGrid::edit(&mut rig.h.ui, g, |v| v.tile(models).map(|t| t.folder)).flatten()
            == Some(true)
    );

    // Thumbnails arrive asynchronously.
    assert!(
        turn_until(&mut rig, |r| {
            let g = part(r, &["grid"]);
            VirtualGrid::edit(&mut r.h.ui, g, |v| v.thumb_of(bricks).is_some()).unwrap_or(false)
        }),
        "the texture's thumbnail never arrived"
    );

    // Rename in place: forge.asset.rename; the id stays, the extension is kept.
    rig.h.ui.raise(
        g,
        RowRenamed {
            view: g,
            key: bricks,
            name: "wall".into(),
        },
    );
    rig.turn();
    rig.turn();
    let path_of = |c: &Rc<RefCell<ServerCatalog>>, key: u64| {
        c.borrow()
            .rows()
            .into_iter()
            .find(|r| r.key & !(1u64 << 63) == key)
            .map(|r| r.path)
    };
    assert_eq!(path_of(&catalog, bricks).as_deref(), Some("wall.png"));
    assert!(
        tiles(&mut rig).iter().any(|(k, _)| *k == bricks),
        "the same tile (same id)"
    );

    // Move it onto the models folder.
    rig.h.ui.raise(
        g,
        RowsDropped {
            view: g,
            keys: vec![bricks],
            target: DropTarget {
                parent: Some(models),
                index: 0,
            },
        },
    );
    rig.turn();
    rig.turn();
    assert_eq!(
        path_of(&catalog, bricks).as_deref(),
        Some("models/wall.png")
    );
    assert!(
        !tiles(&mut rig).iter().any(|(k, _)| *k == bricks),
        "it left the root"
    );

    // Open the folder, remove it (the registration, not the file), undo.
    rig.h.ui.raise(
        g,
        RowActivated {
            view: g,
            key: models,
        },
    );
    rig.turn();
    assert!(tiles(&mut rig).iter().any(|(k, _)| *k == bricks));
    rig.h.ui.raise(
        g,
        RowsDeleteRequested {
            view: g,
            keys: vec![bricks],
        },
    );
    rig.turn();
    rig.turn();
    assert_eq!(path_of(&catalog, bricks), None);
    assert!(
        !tiles(&mut rig).iter().any(|(k, _)| *k == bricks),
        "its tile is gone"
    );
    let has =
        |c: &Rc<RefCell<ServerCatalog>>, p: &str| c.borrow().rows().iter().any(|r| r.path == p);
    rig.shell.emitter().undo();
    rig.turn();
    rig.turn();
    // The registration is back (the file never left); a removed sidecar means the id is
    // minted again from path and content (Ch.8.1), so it is found by path.
    assert!(has(&catalog, "models/wall.png"), "undo restores it");
    assert!(tiles(&mut rig).iter().any(|(_, l)| l == "wall.png"));

    // Undo the move and the rename: the database follows the project back.
    rig.shell.emitter().undo();
    rig.turn();
    rig.shell.emitter().undo();
    rig.turn();
    rig.turn();
    assert_ne!(
        rig.state_hash(),
        start,
        "the session's import is still there"
    );
    assert!(has(&catalog, "bricks.png") && !has(&catalog, "models/wall.png"));
    rig.h.ui.raise(g, GridUp { view: g });
    rig.turn();
    assert!(tiles(&mut rig).iter().any(|(_, l)| l == "bricks.png"));
}

#[test]
fn views_search_and_zero_idle() {
    let (mut rig, _catalog) = server_rig();
    let mut auto = rig.connect(Issuer::Automation {
        session: "s1".into(),
        tool: "import".into(),
    });
    auto.apply(import_command("models/scene.gltf"), None);
    let _ = auto.pump();
    rig.turn();
    rig.turn();
    let g = part(&rig, &["grid"]);
    rig.h.click(part(&rig, &["bar", "mode.list"]));
    rig.turn();
    assert_eq!(
        VirtualGrid::edit(&mut rig.h.ui, g, |v| v.mode()),
        Some(GridMode::List)
    );
    // Search across folders.
    let s = part(&rig, &["bar", "search"]);
    rig.h.ui.raise(
        s,
        SearchChanged {
            field: s,
            query: "scene".into(),
        },
    );
    rig.turn();
    let found = tiles(&mut rig);
    assert!(
        !found.is_empty()
            && found
                .iter()
                .all(|(_, l)| l.to_lowercase().contains("scene")),
        "{found:?}"
    );
    // Let every visible thumbnail arrive, then idle costs nothing.
    turn_until(&mut rig, |r| {
        let g = part(r, &["grid"]);
        VirtualGrid::edit(&mut r.h.ui, g, |v| {
            v.requested_count() == v.resident_thumbs()
        })
        .unwrap_or(false)
    });
    rig.settle();
    // The click above leaves a hover tooltip timer and the search field's timers: let them
    // run out (the D-5 idle budget is measured after them, as `ui_idle_zero_redraw` is).
    rig.advance(Duration::from_secs(6));
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    assert_eq!(
        (frames, wakeups),
        (0, 0),
        "an idle asset browser schedules nothing"
    );
}

/// A catalogue with a locked asset (a teammate's store lock).
struct Locked {
    rows: Vec<AssetRow>,
}

impl AssetCatalog for Locked {
    fn revision(&self) -> u64 {
        1
    }
    fn rows(&self) -> Vec<AssetRow> {
        self.rows.clone()
    }
    fn thumbnail(&self, _key: u64, _size: u32, done: ThumbDone) {
        done(Err("no thumbnails here".into()));
    }
    fn stage(&mut self, _files: &[PathBuf], _folder: &str) -> Result<Vec<String>, EditorError> {
        Ok(Vec::new())
    }
    fn follow(&mut self, _mirror: &ProjectMirror) -> Vec<String> {
        Vec::new()
    }
    fn importable(&self, _path: &str) -> bool {
        false
    }
    fn backend(&self) -> &str {
        "test catalogue"
    }
}

#[test]
fn locked_assets_show_their_holder_and_refuse_moves() {
    let row = |key: u64, name: &str, locked: Option<&str>| AssetRow {
        key,
        id: format!("{key:032x}"),
        path: format!("{name}.png"),
        folder: String::new(),
        name: name.into(),
        kind: "texture".into(),
        locked_by: locked.map(str::to_string),
        generated: false,
        version: 1,
    };
    let cat: Rc<RefCell<dyn AssetCatalog>> = Rc::new(RefCell::new(Locked {
        rows: vec![row(1, "free", None), row(2, "held", Some("bob"))],
    }));
    let mut rig = rig_over(cat);
    let g = part(&rig, &["grid"]);
    let (badge, caption) = VirtualGrid::edit(&mut rig.h.ui, g, |v| {
        v.tile(2)
            .map(|t| (t.badge, t.caption.clone()))
            .unwrap_or_default()
    })
    .unwrap_or_default();
    assert!(badge.is_some(), "a lock badge");
    assert!(caption.contains("locked by bob"), "{caption}");
    assert_eq!(
        VirtualGrid::edit(&mut rig.h.ui, g, |v| v.tile(1).and_then(|t| t.badge)).flatten(),
        None
    );
    let h = rig.shell.mirror().history().len();
    rig.h.ui.raise(
        g,
        RowRenamed {
            view: g,
            key: 2,
            name: "mine".into(),
        },
    );
    rig.turn();
    assert_eq!(
        rig.shell.mirror().history().len(),
        h,
        "no command for a locked asset"
    );
    let warned = rig
        .shell
        .session()
        .notifications
        .history()
        .any(|n| n.detail.contains("bob"));
    assert!(warned, "the refusal names the holder");
}
