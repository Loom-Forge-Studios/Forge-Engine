//! Asset commands on the bus (I7, I8) and Ch.8's DoD seed: import a 500-object glTF scene,
//! mutate the source, see it hot-reload, undo the import — all through commands, which is
//! what an automation session drives.

mod common;

use common::{WAIT, open, p, put_scene, reloaded};
use forge_asset::commands::{self, ADOPT, IMPORT, REMOVE, RENAME, intent_key, invoke};
use forge_asset::fixture::{self, SceneSpec};
use forge_asset::{AssetEvent, AssetServer, MeshAsset, SceneAsset};
use forge_cmd::contract::check_contract;
use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer};
use forge_store::MemoryStore;
use serde_json::json;

fn bus() -> Bus {
    let mut b = Bus::new();
    commands::register_commands(&mut b).expect("registers");
    b
}

fn apply(b: &mut Bus, cmd: EditorCommand) -> forge_cmd::TxnId {
    let issuer = Issuer::Automation {
        session: "auto-1".into(),
        tool: "forge_asset_import".into(),
    };
    let e = b.envelope(issuer, cmd);
    let t = e.txn;
    b.apply(e).expect("applies");
    t
}

#[test]
fn asset_commands_meet_the_command_contract() {
    // I8 over the asset commands: dry run == apply, one undo entry, undo/redo restore the
    // state hash, provenance, serde and reflect round trips.
    let make = || {
        let mut b = bus();
        let e = b.envelope(
            Issuer::Test,
            invoke(IMPORT, &json!({ "source": "models/pre.gltf" })),
        );
        b.apply(e).expect("pre-import");
        b
    };
    let samples = [
        invoke(
            IMPORT,
            &json!({ "source": "models/a.gltf", "settings": { "mips": false } }),
        ),
        invoke(
            RENAME,
            &json!({ "from": "models/pre.gltf", "to": "models/b.gltf" }),
        ),
        invoke(REMOVE, &json!({ "source": "models/pre.gltf" })),
    ];
    let v = check_contract(make, &samples);
    assert!(v.is_empty(), "contract violations: {v:#?}");
}

#[test]
fn bad_arguments_are_refused_with_a_code() {
    let mut b = bus();
    for cmd in [
        invoke(IMPORT, &json!({})),
        invoke(IMPORT, &json!({ "source": "..\\escape.gltf" })),
        invoke(IMPORT, &json!({ "source": "a.gltf", "settings": [1] })),
        invoke(REMOVE, &json!({ "source": "never/imported.gltf" })),
        invoke(
            RENAME,
            &json!({ "from": "never/imported.gltf", "to": "x.gltf" }),
        ),
    ] {
        let e = b.envelope(Issuer::Test, cmd);
        let r = b.apply(e).expect_err("refused");
        assert_eq!(r.error.code().as_str(), "CMD-0011", "{r}");
    }
}

#[test]
fn adopt_records_existing_assets_and_is_not_undoable() {
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "models", &SceneSpec::default());
    let mut a = open(Box::new(s));
    common::import(&mut a, "models/scene.gltf"); // outside the bus (e.g. an older session)
    let mut b = bus();
    let cmd = a.adopt_command(b.project()).expect("something to adopt");
    let t = apply(&mut b, cmd);
    let runs = a.import_runs();
    let ev = a.sync(b.project());
    assert!(ev.is_empty(), "adopting changes nothing on disk: {ev:?}");
    assert_eq!(a.import_runs(), runs);
    assert!(
        b.project()
            .setting(&intent_key("models/scene.gltf"))
            .is_some()
    );
    assert!(
        a.adopt_command(b.project()).is_none(),
        "nothing left to adopt"
    );
    assert!(b.undo(t).is_err(), "{ADOPT} must not be undoable");
}

#[test]
fn dod_seed_500_objects_import_hot_reload_undo_through_commands() {
    let spec = SceneSpec {
        name: "city.gltf".into(),
        objects: 500,
        grid: 4,
        materials: 8,
        textured: true,
        seed: 1,
    };
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "levels", &spec);
    let mut a = open(Box::new(s));
    let mut b = bus();

    // Import, as an automation session would.
    let txn = apply(
        &mut b,
        invoke(IMPORT, &json!({ "source": "levels/city.gltf" })),
    );
    let ev = a.sync(b.project());
    let id = a.id_of(&p("levels/city.gltf")).expect("imported");
    assert!(ev.contains(&AssetEvent::Imported {
        id,
        path: p("levels/city.gltf")
    }));
    let scene = a.load::<SceneAsset>(id).wait(WAIT).expect("scene");
    assert_eq!(scene.nodes.len(), 500);
    assert_eq!(a.assets_of_kind("mesh").len(), 500);
    let m7 = a.load::<MeshAsset>(id.child("mesh:Mesh_7"));
    let before = m7.wait(WAIT).expect("mesh").primitives[0].positions();

    // Mutate the source: every vertex height moves; the live mesh handle reloads.
    let bin = fixture::scene(&SceneSpec {
        seed: 2,
        ..spec.clone()
    })
    .into_iter()
    .find(|(n, _)| n == "city.bin")
    .map(|(_, b)| b)
    .expect("bin");
    common::write(&a, "levels/city.bin", bin);
    let ev = a.poll().expect("polls");
    assert!(reloaded(&ev).contains(&m7.id()), "the live mesh reloaded");
    let after = m7.wait(WAIT).expect("reloaded").primitives[0].positions();
    assert_ne!(before, after);

    // Undo the import: the asset registration goes (the file stays), live handles fail.
    b.undo(txn).expect("undo");
    let ev = a.sync(b.project());
    assert!(
        ev.contains(&AssetEvent::Removed {
            id,
            path: p("levels/city.gltf")
        }),
        "{ev:?}"
    );
    assert!(a.id_of(&p("levels/city.gltf")).is_none());
    assert!(a.assets_of_kind("mesh").is_empty());
    assert!(!a.vfs().exists(&p("levels/city.gltf.meta.ron")).expect("x"));
    assert!(
        a.vfs().exists(&p("levels/city.gltf")).expect("x"),
        "the user's file stays"
    );
    assert!(
        m7.wait(WAIT).is_err(),
        "a handle to a removed asset reports it"
    );

    // Redo: the same id again (minted from the same path and content).
    b.redo(txn).expect("redo");
    a.sync(b.project());
    let again = a.id_of(&p("levels/city.gltf")).expect("re-imported");
    assert_eq!(again, id.child(""), "stable");
    // The audit trail names the automation: provenance for every step.
    assert!(
        b.audit()
            .iter()
            .any(|e| format!("{e:?}").contains("auto-1"))
    );
}

#[test]
fn changing_settings_by_command_reimports_and_undo_restores() {
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "models", &SceneSpec::default());
    let mut a: AssetServer = open(Box::new(s));
    let mut b = bus();
    apply(
        &mut b,
        invoke(IMPORT, &json!({ "source": "models/scene.gltf" })),
    );
    a.sync(b.project());
    let id = a.id_of(&p("models/scene.gltf")).expect("imported");
    let tex = a.load::<forge_asset::TextureAsset>(id.child("texture:Checker"));
    assert_eq!(tex.wait(WAIT).expect("t").levels.len(), 7);
    let t = apply(
        &mut b,
        invoke(
            IMPORT,
            &json!({ "source": "models/scene.gltf", "settings": { "mips": false } }),
        ),
    );
    a.sync(b.project());
    assert_eq!(tex.wait(WAIT).expect("t").levels.len(), 1);
    b.undo(t).expect("undo");
    a.sync(b.project());
    assert_eq!(
        tex.wait(WAIT).expect("t").levels.len(),
        7,
        "undo restores the old settings"
    );
}
