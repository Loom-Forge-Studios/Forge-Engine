//! Hot reload (M2-6, gate row `C-asset-hot-reload`), on the in-memory store and on a real
//! folder (`LocalFs`).
//!
//! One scenario, checked step by step: edit a glTF's `.bin` (same size, different vertices)
//! and the meshes that changed reload under their live handles — and only those; the
//! scene's thumbnail re-renders; rewriting a file with identical bytes reloads nothing; a new
//! texture image reloads the texture; editing the sidecar's settings in a text editor
//! reimports with the new settings.
//!
//! Positive control (W2): `positive_control_length_only_stamps_miss_the_edit` runs the same
//! scenario over a store whose change stamps look only at file length — the cheap shortcut a
//! backend might take — and the scenario must fail at the same-size `.bin` edit.

mod common;

use common::{Backend, WAIT, open, p, put_scene, reloaded, store};
use forge_asset::fixture::{self, SceneSpec};
use forge_asset::{AssetEvent, AssetServer, MeshAsset, SceneAsset, TextureAsset};
use forge_cmd::CommandEnvelope;
use forge_store::{
    Blake3, Bytes, Lock, MemoryStore, ProjectStore, Rev, RevId, RevRange, Stamp, StoreError,
    StorePath, Tree,
};

fn spec(seed: u32) -> SceneSpec {
    SceneSpec {
        name: "ship.gltf".into(),
        objects: 3,
        grid: 2,
        materials: 1,
        textured: true,
        seed,
    }
}

fn file(spec: &SceneSpec, name: &str) -> Bytes {
    fixture::scene(spec)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, b)| b)
        .expect("generated")
}

fn ensure(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

fn scenario(mut s: Box<dyn ProjectStore>) -> Result<(), String> {
    put_scene(s.as_mut(), "models", &spec(1));
    let mut a = open(s);
    let id = common::import(&mut a, "models/ship.gltf");
    let e = |e: forge_asset::AssetError| e.to_string();

    let scene = a.load::<SceneAsset>(id);
    let meshes: Vec<_> = (0..3)
        .map(|i| a.load::<MeshAsset>(id.child(&format!("mesh:Mesh_{i}"))))
        .collect();
    let tex = a.load::<TextureAsset>(id.child("texture:Checker"));
    let thumb = a.thumbnail(id, 32);
    scene.wait(WAIT).map_err(e)?;
    let before: Vec<_> = meshes
        .iter()
        .map(|m| m.wait(WAIT).map(|v| v.primitives[0].positions()))
        .collect::<Result<_, _>>()
        .map_err(e)?;
    tex.wait(WAIT).map_err(e)?;
    let thumb0 = thumb.wait(WAIT).map_err(e)?;
    ensure(
        thumb0.rgba.chunks(4).any(|px| px[3] == 255),
        "the scene thumbnail draws something",
    )?;

    // Nothing changed since the import: a poll is silent.
    let ev = a.poll().map_err(e)?;
    ensure(ev.is_empty(), &format!("a quiet poll reported {ev:?}"))?;

    // 1. The .bin changes (same length, other vertices): the glTF reimports.
    let old_bin = a.vfs().read(&p("models/ship.bin")).map_err(e)?;
    let new_bin = file(&spec(2), "ship.bin");
    ensure(
        old_bin.len() == new_bin.len() && old_bin != new_bin,
        "fixture: same-size edit",
    )?;
    common::write(&a, "models/ship.bin", new_bin);
    let ev = a.poll().map_err(e)?;
    ensure(
        ev.contains(&AssetEvent::Reimported {
            id,
            path: p("models/ship.gltf"),
        }),
        "the .bin edit was not seen (no reimport)",
    )?;
    let changed = reloaded(&ev);
    let mut moved = 0;
    for (m, old) in meshes.iter().zip(&before) {
        let now = m.wait(WAIT).map_err(e)?.primitives[0].positions();
        if now != *old {
            moved += 1;
            ensure(
                changed.contains(&m.id()),
                "a changed mesh reported no reload",
            )?;
            ensure(
                m.generation() == 2,
                "a changed mesh's handle was not swapped",
            )?;
        } else {
            ensure(!changed.contains(&m.id()), "an unchanged mesh reloaded")?;
            ensure(
                m.generation() == 1,
                "an unchanged mesh's handle was swapped",
            )?;
        }
    }
    ensure(moved > 0, "the edit moved no vertex")?;
    ensure(
        tex.generation() == 1,
        "the texture reloaded though it did not change",
    )?;
    ensure(
        !changed.contains(&id),
        "the scene's nodes did not change, it must not reload",
    )?;
    thumb.wait(WAIT).map_err(e)?;
    ensure(
        thumb.generation() == 2,
        "the scene thumbnail did not re-render",
    )?;

    // 2. Identical bytes rewritten: nothing reimports, nothing reloads.
    let runs = a.import_runs();
    let same = a.vfs().read(&p("models/ship.bin")).map_err(e)?;
    common::write(&a, "models/ship.bin", same);
    let ev = a.poll().map_err(e)?;
    ensure(
        reloaded(&ev).is_empty() && a.import_runs() == runs,
        &format!("an identical rewrite caused work: {ev:?}"),
    )?;

    // 3. A new texture image: the texture reloads with the new pixels.
    common::write(
        &a,
        "models/textures/checker.png",
        fixture::checker_png(64, [10, 200, 30, 255], [0, 0, 0, 255]),
    );
    let ev = a.poll().map_err(e)?;
    ensure(
        reloaded(&ev).contains(&tex.id()),
        "the texture did not reload",
    )?;
    let t = tex.wait(WAIT).map_err(e)?;
    ensure(
        t.levels[0][..4] == [10, 200, 30, 255],
        "the texture shows old pixels",
    )?;

    // 4. The sidecar's settings edited by hand: reimport with them (no mips).
    let scp = p("models/ship.gltf.meta.ron");
    let text =
        String::from_utf8(a.vfs().read(&scp).map_err(e)?.to_vec()).map_err(|x| x.to_string())?;
    let edited = text.replacen("settings: {}", "settings: {\"mips\": \"false\"}", 1);
    ensure(
        edited != text,
        "fixture: the sidecar has an empty settings map",
    )?;
    common::write(&a, "models/ship.gltf.meta.ron", edited);
    let ev = a.poll().map_err(e)?;
    ensure(
        ev.contains(&AssetEvent::Reimported {
            id,
            path: p("models/ship.gltf"),
        }),
        &format!("a settings edit did not reimport: {ev:?}"),
    )?;
    let t = tex.wait(WAIT).map_err(e)?;
    ensure(
        t.levels.len() == 1,
        "the new setting (mips: false) was not applied",
    )?;
    Ok(())
}

#[test]
fn hot_reload_on_the_memory_store() {
    scenario(store(&Backend::Memory)).expect("hot reload works");
}

#[test]
fn hot_reload_on_a_real_folder() {
    scenario(store(&Backend::Local("hot"))).expect("hot reload works on LocalFs");
}

#[test]
fn a_quiet_project_polls_without_reading_files() {
    // The poll is one stamps() call; with nothing changed it returns no events and runs no
    // importer (checked above per step). Here: many polls stay silent and cheap.
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "models", &spec(1));
    let mut a = open(Box::new(s));
    common::import(&mut a, "models/ship.gltf");
    let runs = a.import_runs();
    for _ in 0..100 {
        assert!(a.poll().expect("polls").is_empty());
    }
    assert_eq!(a.import_runs(), runs);
}

// ---- positive control ----------------------------------------------------------------

/// A store whose change stamps see only the file length.
struct LengthStamps(MemoryStore);

impl ProjectStore for LengthStamps {
    fn backend(&self) -> &'static str {
        "length-stamps"
    }
    fn identity(&self) -> &str {
        self.0.identity()
    }
    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        self.0.read(path)
    }
    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError> {
        self.0.write(path, b)
    }
    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        self.0.delete(path)
    }
    fn list(&self) -> Result<Vec<StorePath>, StoreError> {
        self.0.list()
    }
    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError> {
        self.0.blob_get(h)
    }
    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError> {
        self.0.blob_put(b)
    }
    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError> {
        self.0.blob_has(h)
    }
    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId, StoreError> {
        self.0.commit(msg, envelopes)
    }
    fn head(&self) -> Result<Option<RevId>, StoreError> {
        self.0.head()
    }
    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError> {
        self.0.history(range)
    }
    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError> {
        self.0.tree(rev)
    }
    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError> {
        self.0.commands(rev)
    }
    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError> {
        self.0.lock(path)
    }
    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError> {
        self.0.unlock(lock)
    }
    fn locks(&self) -> Result<Vec<Lock>, StoreError> {
        self.0.locks()
    }
    fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        let mut out = Vec::new();
        for p in self.0.list()? {
            let n = self.0.read(&p)?.len() as u64;
            out.push((p, Stamp::of_metadata(&[n])));
        }
        Ok(out)
    }
}

#[test]
fn positive_control_length_only_stamps_miss_the_edit() {
    let err = scenario(Box::new(LengthStamps(MemoryStore::new("ada"))))
        .expect_err("a length-only stamp cannot see a same-size edit");
    assert!(err.contains("the .bin edit was not seen"), "{err}");
}

#[test]
fn deleting_a_source_keeps_its_last_value_and_its_return_restores_it() {
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "models", &spec(1));
    let mut a = open(Box::new(s));
    let id = common::import(&mut a, "models/ship.gltf");
    let h = a.load::<SceneAsset>(id);
    h.wait(WAIT).expect("loads");
    let gltf = a.vfs().read(&p("models/ship.gltf")).expect("read");
    a.vfs().delete(&p("models/ship.gltf")).expect("delete");
    let ev = a.poll().expect("polls");
    assert!(
        ev.iter().any(|e| matches!(e, AssetEvent::Failed { .. })),
        "the missing source is reported: {ev:?}"
    );
    assert!(h.get().is_some(), "the last value stays visible");
    common::write(&a, "models/ship.gltf", gltf);
    a.poll().expect("polls");
    assert_eq!(
        a.id_of(&p("models/ship.gltf")),
        Some(id),
        "same id on return"
    );
}

#[allow(dead_code)]
fn _server_is_send(a: AssetServer) -> impl Send {
    a
}
