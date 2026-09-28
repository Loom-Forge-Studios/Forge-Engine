//! Asset sources beyond glTF: generated assets (a `SeedPath`, no file — Ch.8 brief),
//! standalone PNG and KTX2 textures, the async handle future, and thumbnails of every
//! built-in kind.

mod common;

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use common::{WAIT, open, p, put_scene};
use forge_asset::fixture::{self, SceneSpec};
use forge_asset::types::AssetValue;
use forge_asset::{AssetId, AssetSource, MaterialAsset, TextureAsset, texture_from_rgba8};
use forge_seed::{Seed, SeedPath};
use forge_store::{MemoryStore, ProjectStore};

/// A deterministic procedural texture: an 8x8 tile coloured from the seed its path resolves to.
fn noise_tile(path: &SeedPath) -> Result<forge_store::Bytes, String> {
    let v = path.resolve(Seed::root(42)).raw();
    let rgba: Vec<u8> = (0..64u64)
        .flat_map(|i| {
            let x = forge_seed::splitmix64(v ^ i);
            [x as u8, (x >> 8) as u8, (x >> 16) as u8, 255]
        })
        .collect();
    texture_from_rgba8(rgba, 8, 8, true, true).encode()
}

#[test]
fn generated_assets_are_ids_handles_and_thumbnails_without_files() {
    let mut a = open(Box::new(MemoryStore::new("ada")));
    a.register_generator("noise", "texture", Arc::new(noise_tile));
    let seed = SeedPath::universe().child("body", 3).child("region", 17);
    let id = a.generated("noise", &seed).expect("registered");
    assert_eq!(
        id,
        AssetId::generated("noise", &seed),
        "the id is the path's, everywhere"
    );
    assert!(matches!(
        a.info(id).map(|i| &i.source),
        Some(AssetSource::Generated { .. })
    ));
    let t = a
        .load::<TextureAsset>(id)
        .wait(WAIT)
        .expect("generated on a worker");
    assert_eq!((t.width, t.levels.len()), (8, 4));
    let again = noise_tile(&seed).expect("pure");
    assert_eq!(
        t.encode().expect("encodes"),
        again,
        "deterministic: the same path gives the same bytes"
    );
    let other = a
        .generated("noise", &SeedPath::universe().child("body", 4))
        .expect("another");
    let t2 = a.load::<TextureAsset>(other).wait(WAIT).expect("loads");
    assert_ne!(
        t.levels[0], t2.levels[0],
        "a different path is a different asset"
    );
    let th = a.thumbnail(id, 16).wait(WAIT).expect("thumbnail");
    assert_eq!(th.rgba.len(), 16 * 16 * 4);
    // Never stored: the project has no files and no blobs for it.
    assert!(a.vfs().list().expect("list").is_empty());
    assert!(a.path_of(id).is_none());
    // An unknown generator is a coded error, not a panic.
    assert_eq!(
        a.generated("nope", &seed)
            .expect_err("unknown")
            .code()
            .as_str(),
        "ASSET-0014"
    );
}

#[test]
fn standalone_png_and_ktx2_textures_import() {
    let mut s = MemoryStore::new("ada");
    s.write(
        &p("tex/brick.png"),
        fixture::checker_png(16, [200, 100, 50, 255], [20, 20, 20, 255]),
    )
    .expect("w");
    let ktx = texture_from_rgba8(vec![7; 4 * 4 * 4], 4, 4, false, true)
        .encode()
        .expect("encodes");
    s.write(&p("tex/data.ktx2"), ktx.clone()).expect("w");
    s.write(
        &p("tex/bad.ktx2"),
        forge_store::Bytes::from_static(b"not ktx"),
    )
    .expect("w");
    let mut a = open(Box::new(s));
    assert_eq!(a.importer_for(&p("tex/brick.png")), Some("image"));
    assert_eq!(a.importer_for(&p("tex/brick.PNG")), Some("image"));
    assert_eq!(a.importer_for(&p("tex/brick.png.meta.ron")), None);
    let png = common::import(&mut a, "tex/brick.png");
    let t = a.load::<TextureAsset>(png).wait(WAIT).expect("png");
    assert_eq!((t.width, t.srgb, t.levels.len()), (16, true, 5));
    let k = common::import(&mut a, "tex/data.ktx2");
    let blob = a.info(k).and_then(|i| i.blob).expect("stored");
    assert_eq!(
        a.vfs().blob_get(blob).expect("blob"),
        ktx,
        "KTX2 passes through byte for byte"
    );
    let mut ev = Vec::new();
    let e = a
        .import(&p("tex/bad.ktx2"), Default::default(), &mut ev)
        .expect_err("invalid KTX2");
    assert_eq!(e.code().as_str(), "ASSET-0005");
    // Same pixels imported twice are stored once (content addressing).
    let mut s2 = MemoryStore::new("ada");
    let bytes = fixture::checker_png(16, [1, 2, 3, 255], [4, 5, 6, 255]);
    s2.write(&p("a.png"), bytes.clone()).expect("w");
    s2.write(&p("b.png"), bytes).expect("w");
    let mut b = open(Box::new(s2));
    let (x, y) = (
        common::import(&mut b, "a.png"),
        common::import(&mut b, "b.png"),
    );
    assert_ne!(x, y, "two files, two identities");
    assert_eq!(
        b.info(x).and_then(|i| i.blob),
        b.info(y).and_then(|i| i.blob)
    );
}

struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(WAIT),
        }
    }
}

#[test]
fn handles_are_awaitable() {
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "m", &SceneSpec::default());
    let mut a = open(Box::new(s));
    let id = common::import(&mut a, "m/scene.gltf");
    let mat = block_on(
        a.load::<MaterialAsset>(id.child("material:Material_0"))
            .ready(),
    )
    .expect("awaits the worker");
    assert_eq!(mat.name, "Material_0");
    let missing = block_on(a.load::<MaterialAsset>(AssetId(99)).ready());
    assert_eq!(missing.expect_err("unknown").code().as_str(), "ASSET-0006");
}

#[test]
fn every_builtin_kind_has_a_thumbnail() {
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "m", &SceneSpec::default());
    let mut a = open(Box::new(s));
    let id = common::import(&mut a, "m/scene.gltf");
    for (what, aid) in [
        ("scene", id),
        ("mesh", id.child("mesh:Mesh_0")),
        ("material", id.child("material:Material_0")),
        ("texture", id.child("texture:Checker")),
    ] {
        let t = a
            .thumbnail(aid, 24)
            .wait(WAIT)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(t.size, 24);
        let opaque = t.rgba.chunks(4).filter(|px| px[3] == 255).count();
        assert!(
            opaque > 24,
            "{what}: the thumbnail shows something ({opaque} px)"
        );
    }
    // Thumbnails are deterministic: the same asset renders the same pixels.
    let (x, y) = (
        a.thumbnail(id, 40).wait(WAIT).expect("t"),
        a.thumbnail(id, 41).wait(WAIT).expect("t"),
    );
    assert_ne!(x.rgba.len(), y.rgba.len());
    let again = forge_asset::render_thumbnail(
        "scene",
        &a.vfs()
            .blob_get(a.info(id).and_then(|i| i.blob).expect("b"))
            .expect("bytes"),
        &|m| {
            a.info(m)
                .and_then(|i| i.blob)
                .and_then(|b| a.vfs().blob_get(b).ok())
        },
        40,
    )
    .expect("renders");
    assert_eq!(again, *x);
}
