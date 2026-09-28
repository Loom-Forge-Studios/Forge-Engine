//! References inside assets resolve exactly (M0-17 integration, gate row
//! `C-asset-ref-exact-case`).
//!
//! A glTF whose `uri` names `Textures/Checker.PNG` while the file is
//! `textures/checker.png` imports on Windows if the importer asks the filesystem (NTFS is
//! case-insensitive) and fails on Linux. forge-asset resolves against the store's listing
//! by exact name, so it fails everywhere with `ASSET-0002`, naming the file as it is on disk;
//! a backslash `uri` is `ASSET-0001`. Checked on a real folder (`LocalFs`), in memory, and on
//! a store that answers reads case-insensitively the way NTFS does (`CaseInsensitive`, so the
//! Windows behaviour is exercised on every platform).
//!
//! Positive control (W2): `positive_control_a_naive_importer_breaks_the_rule` runs the same
//! scenario — same files, same case-insensitive store, same `check_asset_0002` assertion —
//! with the glTF importer replaced (through the ordinary `Registry::replace`) by a mutant
//! that resolves each `uri` by asking the store for the joined path directly, instead of
//! through `ImportCx::dependency`. The storage finds the wrong-case file, the import
//! succeeds, and the assertion must fail. A checker that passed there would be vacuous.
//!
//! `the_refusal_is_the_rule_not_the_platform` adds the real-disk side: on Windows the OS
//! itself opens the wrong-case path, and forge-asset still refuses it.

mod common;

use std::sync::Arc;

use common::{Backend, open, p, put_scene, store, tmp_dir};
use forge_asset::fixture::SceneSpec;
use forge_asset::{
    AssetError, AssetId, AssetServer, ImportCx, Importer, ImporterPoint, Settings, SharedStore, Vfs,
};
use forge_cmd::CommandEnvelope;
use forge_plugin::{ItemId, PluginId};
use forge_store::{
    Blake3, Bytes, LocalFs, Lock, MemoryStore, ProjectStore, Rev, RevId, RevRange, StoreError,
    StorePath, Tree,
};

fn spec() -> SceneSpec {
    SceneSpec {
        name: "ship.gltf".into(),
        objects: 1,
        grid: 1,
        materials: 1,
        textured: true,
        seed: 0,
    }
}

fn with_uri(s: &mut dyn ProjectStore, from: &str, to: &str) {
    let path = p("models/ship.gltf");
    let text = String::from_utf8(s.read(&path).expect("gltf").to_vec()).expect("utf8");
    assert!(text.contains(from), "fixture names {from}");
    s.write(&path, Bytes::from(text.replace(from, to)))
        .expect("write");
}

fn import_err(s: Box<dyn ProjectStore>) -> AssetError {
    let mut a = open(s);
    let mut ev = Vec::new();
    a.import(&p("models/ship.gltf"), Settings::new(), &mut ev)
        .expect_err("the reference must not resolve")
}

/// The scene with its texture reference spelled in the wrong case.
fn wrong_case(s: &mut dyn ProjectStore) {
    put_scene(s, "models", &spec());
    with_uri(s, "\"textures/checker.png\"", "\"Textures/Checker.PNG\"");
}

/// Import the wrong-case scene from `vfs` with the importers in `x`.
fn import_wrong_case(vfs: Arc<Vfs>, x: &forge_plugin::Extensions) -> Result<AssetId, AssetError> {
    let (mut a, _) = AssetServer::open_vfs(vfs, x).expect("opens");
    let mut ev = Vec::new();
    a.import(&p("models/ship.gltf"), Settings::new(), &mut ev)
}

/// The rule, as one check: the import fails with `ASSET-0002`, naming the reference as
/// written, the file as it is on disk, and the file that referenced it.
fn check_asset_0002(r: Result<AssetId, AssetError>) -> Result<(), String> {
    match r {
        Err(AssetError::CaseMismatch {
            reference,
            on_disk,
            from,
        }) => {
            if reference == "Textures/Checker.PNG"
                && on_disk == "models/textures/checker.png"
                && from == "models/ship.gltf"
            {
                Ok(())
            } else {
                Err(format!(
                    "ASSET-0002 with the wrong names: {reference} / {on_disk} / {from}"
                ))
            }
        }
        Err(other) => Err(format!("expected ASSET-0002, got {other}")),
        Ok(id) => Err(format!(
            "expected ASSET-0002, but the wrong-case reference imported (as {id})"
        )),
    }
}

#[test]
fn a_wrong_case_uri_fails_on_every_backend() {
    let x = forge_importers::asset_extensions().expect("extensions");
    for b in [Backend::Memory, Backend::Local("refcase")] {
        let mut s = store(&b);
        wrong_case(s.as_mut());
        check_asset_0002(import_wrong_case(Arc::new(Vfs::new(s)), &x)).expect("the rule holds");
    }
    let mut s = CaseInsensitive(MemoryStore::new("ada"));
    wrong_case(&mut s);
    check_asset_0002(import_wrong_case(Arc::new(Vfs::new(Box::new(s))), &x))
        .expect("the rule holds on a case-insensitive store");
}

#[test]
fn a_wrong_case_directory_fails_too() {
    let mut s = store(&Backend::Local("refcase-dir"));
    put_scene(s.as_mut(), "models", &spec());
    with_uri(s.as_mut(), "\"ship.bin\"", "\"../Models/ship.bin\"");
    assert_eq!(import_err(s).code().as_str(), "ASSET-0002");
}

#[test]
fn a_backslash_uri_fails_on_every_backend() {
    for b in [Backend::Memory, Backend::Local("refslash")] {
        let mut s = store(&b);
        put_scene(s.as_mut(), "models", &spec());
        with_uri(
            s.as_mut(),
            "\"textures/checker.png\"",
            "\"textures\\\\checker.png\"",
        );
        assert_eq!(import_err(s).code().as_str(), "ASSET-0001");
    }
}

#[test]
fn the_correct_spelling_imports() {
    let mut s = store(&Backend::Local("refok"));
    put_scene(s.as_mut(), "models", &spec());
    let mut a = open(s);
    common::import(&mut a, "models/ship.gltf");
}

#[test]
fn the_refusal_is_the_rule_not_the_platform() {
    let dir = tmp_dir("refcase-os");
    let mut s = LocalFs::open(&dir, "ada").expect("local fs");
    put_scene(&mut s, "models", &spec());
    let wrong = dir.join("models").join("Textures").join("Checker.PNG");
    let os_opens = std::fs::read(&wrong).is_ok();
    // The OS answer depends on the filesystem; the rule's answer must not.
    if cfg!(windows) {
        assert!(
            os_opens,
            "NTFS resolves the wrong-case path: the bug class is live here"
        );
    }
    with_uri(
        &mut s,
        "\"textures/checker.png\"",
        "\"Textures/Checker.PNG\"",
    );
    assert_eq!(import_err(Box::new(s)).code().as_str(), "ASSET-0002");
}

// ---- positive control ----------------------------------------------------------------

#[test]
fn positive_control_a_naive_importer_breaks_the_rule() {
    let mut s = CaseInsensitive(MemoryStore::new("ada"));
    wrong_case(&mut s);
    let vfs = Arc::new(Vfs::new(Box::new(s)));
    let mut x = forge_importers::asset_extensions().expect("extensions");
    x.registry_mut::<ImporterPoint>()
        .expect("defined")
        .replace(
            &PluginId::new("test.mutant").expect("id"),
            &ItemId::of::<ImporterPoint>("gltf").expect("item"),
            Arc::new(NaiveGltf(Arc::clone(vfs.store()))) as Arc<dyn Importer>,
        )
        .expect("replaces");
    let err = check_asset_0002(import_wrong_case(vfs, &x))
        .expect_err("a resolver that asks the storage must be caught");
    assert!(err.contains("imported"), "{err}");
}

/// The mutant: resolves each `uri` by joining it onto the glTF's folder and reading that
/// path from the store directly — what an importer does when it asks the filesystem.
struct NaiveGltf(SharedStore);

impl Importer for NaiveGltf {
    fn version(&self) -> u32 {
        1
    }
    fn extensions(&self) -> &[&str] {
        &["gltf"]
    }
    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        let doc: serde_json::Value = serde_json::from_slice(cx.source()).map_err(|e| cx.fail(e))?;
        let mut dir: Vec<String> = cx.path().segments().map(str::to_string).collect();
        dir.pop();
        let mut uris = Vec::new();
        for key in ["buffers", "images"] {
            for item in doc[key].as_array().into_iter().flatten() {
                if let Some(uri) = item["uri"].as_str() {
                    uris.push(uri.to_string());
                }
            }
        }
        assert!(
            uris.iter().any(|u| u == "Textures/Checker.PNG"),
            "the mutant sees the wrong-case reference: {uris:?}"
        );
        let store = self.0.read().expect("store");
        for (i, uri) in uris.iter().enumerate() {
            let joined = format!("{}/{uri}", dir.join("/"));
            let path = StorePath::new(&joined).map_err(|e| cx.fail(e))?;
            let bytes = store.read(&path).map_err(|e| cx.fail(e))?;
            cx.emit(&format!("dep:{i}"), "texture", uri, bytes);
        }
        drop(store);
        let source = cx.source().clone();
        cx.emit("", "scene", "ship", source);
        Ok(())
    }
}

/// A store that finds files regardless of case, as NTFS does: a read of a path that is not
/// there exactly returns the file whose name differs only by case. (Listings stay exact.)
struct CaseInsensitive(MemoryStore);

impl ProjectStore for CaseInsensitive {
    fn backend(&self) -> &'static str {
        "case-insensitive"
    }
    fn identity(&self) -> &str {
        self.0.identity()
    }
    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        match self.0.read(path) {
            Err(StoreError::NotFound(_)) => {
                let want = path.folded();
                match self.0.list()?.into_iter().find(|q| q.folded() == want) {
                    Some(q) => self.0.read(&q),
                    None => Err(StoreError::NotFound(path.to_string())),
                }
            }
            r => r,
        }
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
}
