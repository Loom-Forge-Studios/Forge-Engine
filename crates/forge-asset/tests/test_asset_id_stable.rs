//! `AssetId` is stable across rename (M2-6, gate row `C-asset-id-stable`).
//!
//! Every way a file can move keeps its id and its sub-asset ids, so a scene's references to
//! its meshes (and anything else referencing them) survive:
//!
//! * `AssetServer::rename` (the API the command uses), including a case-only rename on a real
//!   Windows/Linux folder;
//! * the bus command `forge.asset.rename`, then undo (the file moves back) and redo;
//! * outside the editor: the source and its sidecar moved together (a pair rename, what a
//!   file manager or `git mv` of both does), and the source moved **alone** — the sidecar is
//!   re-attached by content;
//! * closing and reopening the project after all of it.
//!
//! While the server watches, even a move that loses the sidecar keeps the id (the vanished
//! source is matched to the new file by content).
//!
//! Positive control (W2): `positive_control_losing_the_sidecar_loses_the_id` moves a source
//! and deletes its sidecar **with the editor closed**; re-importing mints a new id and the
//! stability check must fail — proving the check compares real ids, and that the sidecar is
//! what carries identity across sessions.

mod common;

use common::{Backend, WAIT, open, p, put_scene, store};
use forge_asset::commands::{self, RENAME, invoke, register_commands};
use forge_asset::fixture::SceneSpec;
use forge_asset::{AssetEvent, AssetId, AssetServer, SceneAsset, Settings};
use forge_cmd::{Bus, CommandSink, Issuer};

fn spec() -> SceneSpec {
    SceneSpec {
        name: "ship.gltf".into(),
        objects: 2,
        grid: 1,
        materials: 1,
        textured: false,
        seed: 0,
    }
}

/// The id of `path` now equals `id`, its scene still references its own meshes, and those
/// load.
fn check_same(a: &AssetServer, path: &str, id: AssetId) -> Result<(), String> {
    let now = a
        .id_of(&p(path))
        .ok_or_else(|| format!("{path} is not an imported asset"))?;
    if now != id {
        return Err(format!("{path} has id {now}, it had {id}"));
    }
    let scene = a
        .load::<SceneAsset>(id)
        .wait(WAIT)
        .map_err(|e| e.to_string())?;
    for n in &scene.nodes {
        let m = n.mesh.ok_or("a node lost its mesh")?;
        if m != id.child(&format!(
            "mesh:Mesh_{}",
            n.name.trim_start_matches("Object_")
        )) {
            return Err(format!("{} references {m}, not its own mesh", n.name));
        }
        if a.path_of(m) != Some(&p(path)) {
            return Err(format!("mesh {m} is not attributed to {path}"));
        }
    }
    Ok(())
}

#[test]
fn the_api_rename_keeps_every_id() {
    for b in [Backend::Memory, Backend::Local("rename-api")] {
        let mut s = store(&b);
        put_scene(s.as_mut(), "models", &spec());
        let mut a = open(s);
        let id = common::import(&mut a, "models/ship.gltf");
        let mut ev = Vec::new();
        a.rename(&p("models/ship.gltf"), &p("vehicles/hull.gltf"), &mut ev)
            .expect("renames");
        assert!(
            a.vfs()
                .exists(&p("vehicles/hull.gltf.meta.ron"))
                .expect("x")
        );
        assert!(!a.vfs().exists(&p("models/ship.gltf")).expect("x"));
        check_same(&a, "vehicles/hull.gltf", id).expect("stable");
        // Case-only: on NTFS the old name is the new one until the rename; must still work.
        a.rename(&p("vehicles/hull.gltf"), &p("vehicles/Hull.gltf"), &mut ev)
            .expect("case-only rename");
        check_same(&a, "vehicles/Hull.gltf", id).expect("stable across a case-only rename");
        assert_eq!(
            a.vfs()
                .list()
                .expect("l")
                .iter()
                .filter(|f| f.as_str().starts_with("vehicles/"))
                .count(),
            2
        );
        // The moved glTF's buffer reference is relative and still resolves: a reimport of the
        // moved source must find models/ship.bin? No: it moved away from it — that is honest.
        let r = a.reimport(&p("vehicles/Hull.gltf"), &mut ev);
        assert!(r.is_ok(), "fresh record: nothing to reimport ({r:?})");
    }
}

#[test]
fn the_rename_command_undoes_and_redoes() {
    let mut s = store(&Backend::Memory);
    put_scene(s.as_mut(), "models", &spec());
    let mut a = open(s);
    let mut bus = Bus::new();
    register_commands(&mut bus).expect("registers");
    let me = Issuer::Human { user: "ada".into() };
    let imp = bus.envelope(
        me.clone(),
        invoke(
            commands::IMPORT,
            &serde_json::json!({ "source": "models/ship.gltf" }),
        ),
    );
    bus.apply(imp).expect("import applies");
    a.sync(bus.project());
    let id = a
        .id_of(&p("models/ship.gltf"))
        .expect("imported by command");

    let ren = bus.envelope(
        me.clone(),
        invoke(
            RENAME,
            &serde_json::json!({ "from": "models/ship.gltf", "to": "props/boat.gltf" }),
        ),
    );
    let txn = ren.txn;
    // Dry run first: the preview changes nothing.
    let preview = bus.dry_run(&ren).expect("dry run");
    assert_eq!(preview.changes.len(), 2);
    assert!(a.sync(bus.project()).is_empty());
    bus.apply(ren).expect("rename applies");
    let ev = a.sync(bus.project());
    assert!(
        ev.iter().any(|e| matches!(e, AssetEvent::Renamed { .. })),
        "{ev:?}"
    );
    check_same(&a, "props/boat.gltf", id).expect("stable after the command");

    bus.undo(txn).expect("undo");
    a.sync(bus.project());
    check_same(&a, "models/ship.gltf", id).expect("undo moves it back, same id");
    assert!(!a.vfs().exists(&p("props/boat.gltf")).expect("x"));

    bus.redo(txn).expect("redo");
    a.sync(bus.project());
    check_same(&a, "props/boat.gltf", id).expect("redo moves it again, same id");
}

#[test]
fn a_rename_outside_the_editor_keeps_the_id() {
    let mut s = store(&Backend::Local("rename-outside"));
    put_scene(s.as_mut(), "models", &spec());
    let mut a = open(s);
    let id = common::import(&mut a, "models/ship.gltf");
    let vfs = a.vfs().clone();
    let mv = |from: &str, to: &str| {
        let b = vfs.read(&p(from)).expect("read");
        vfs.write(&p(to), b).expect("write");
        vfs.delete(&p(from)).expect("delete");
    };
    // Source and sidecar together.
    mv("models/ship.gltf", "models/a.gltf");
    mv("models/ship.gltf.meta.ron", "models/a.gltf.meta.ron");
    let ev = a.poll().expect("polls");
    assert!(
        ev.iter().any(|e| matches!(e, AssetEvent::Renamed { .. })),
        "{ev:?}"
    );
    check_same(&a, "models/a.gltf", id).expect("pair rename");
    // The source alone: its sidecar stays behind and is re-attached by content.
    mv("models/a.gltf", "models/b.gltf");
    let ev = a.poll().expect("polls");
    assert!(
        ev.iter().any(|e| matches!(e, AssetEvent::Renamed { .. })),
        "{ev:?}"
    );
    check_same(&a, "models/b.gltf", id).expect("source-only rename");
    assert!(
        a.vfs().exists(&p("models/b.gltf.meta.ron")).expect("x"),
        "the sidecar followed"
    );
    assert!(!a.vfs().exists(&p("models/a.gltf.meta.ron")).expect("x"));
    // Moved, and the sidecar deleted, while the server watches: matched by content.
    mv("models/b.gltf", "models/c.gltf");
    vfs.delete(&p("models/b.gltf.meta.ron"))
        .expect("sidecar lost");
    a.poll().expect("polls");
    check_same(&a, "models/c.gltf", id).expect("a watched move survives losing the sidecar");
    assert!(
        a.vfs().exists(&p("models/c.gltf.meta.ron")).expect("x"),
        "rewritten"
    );
}

#[test]
fn ids_survive_closing_and_reopening() {
    let mut s = store(&Backend::Memory);
    put_scene(s.as_mut(), "models", &spec());
    let mut a = open(s);
    let id = common::import(&mut a, "models/ship.gltf");
    let mut ev = Vec::new();
    a.rename(&p("models/ship.gltf"), &p("models/renamed.gltf"), &mut ev)
        .expect("renames");
    // Reopen over the same store: every id comes from the sidecars, no importer runs.
    let shared = a.vfs().clone();
    drop(a);
    let (b, _) = AssetServer::open_vfs(shared, &forge_importers::asset_extensions().expect("x"))
        .expect("reopens");
    check_same(&b, "models/renamed.gltf", id).expect("same id after reopening");
    assert_eq!(
        b.import_runs(),
        0,
        "a fresh record means no importer runs at open"
    );
}

#[test]
fn positive_control_losing_the_sidecar_loses_the_id() {
    let mut s = store(&Backend::Memory);
    put_scene(s.as_mut(), "models", &spec());
    let mut a = open(s);
    let id = common::import(&mut a, "models/ship.gltf");
    // With the editor closed (nobody watching), the file moves and its sidecar is lost.
    let vfs = a.vfs().clone();
    drop(a);
    let b = vfs.read(&p("models/ship.gltf")).expect("read");
    vfs.delete(&p("models/ship.gltf.meta.ron"))
        .expect("sidecar deleted");
    vfs.delete(&p("models/ship.gltf")).expect("source deleted");
    vfs.write(&p("models/other.gltf"), b)
        .expect("written elsewhere");
    let (mut a, _) = AssetServer::open_vfs(vfs, &forge_importers::asset_extensions().expect("x"))
        .expect("opens");
    let mut ev = Vec::new();
    a.import(&p("models/other.gltf"), Settings::new(), &mut ev)
        .expect("imports anew");
    let err =
        check_same(&a, "models/other.gltf", id).expect_err("without its sidecar the id is new");
    assert!(err.contains("has id"), "{err}");
}
