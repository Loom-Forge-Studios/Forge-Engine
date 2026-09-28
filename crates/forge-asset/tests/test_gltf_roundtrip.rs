//! glTF import round trip (M2-6, gate row `C-asset-gltf-roundtrip`).
//!
//! A generated glTF scene (external `.bin`, external PNG textures, several materials) is
//! imported; its assets are checked against what the generator wrote; the scene is exported
//! to `.glb` and the `.glb` re-imported; the two asset families must be identical asset for
//! asset (compared by label, with ids — which differ because the sources differ — mapped to
//! labels). The same scene with its buffer as a `data:` URI imports to the same assets too.
//!
//! Positive control (W2): `positive_control_a_lossy_exporter_fails_the_round_trip` chains a
//! mutant onto the registered glTF exporter (through the ordinary `Registry::chain`) that
//! drops vertex normals; the round-trip comparison must fail and name a mesh.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use base64::Engine as _;
use common::{WAIT, open, p, put_scene};
use forge_asset::fixture::SceneSpec;
use forge_asset::types::{AssetValue, Semantic};
use forge_asset::{
    AssetId, AssetServer, ExportArtefact, ExportSource, Exporter, ExporterPoint, MaterialAsset,
    MeshAsset, SceneAsset, Settings, TextureAsset,
};
use forge_plugin::{ItemId, PluginId};
use forge_store::{Blake3, Bytes, MemoryStore, ProjectStore};

fn spec() -> SceneSpec {
    SceneSpec {
        name: "scene.gltf".into(),
        objects: 6,
        grid: 3,
        materials: 3,
        textured: true,
        seed: 7,
    }
}

/// label -> canonical artefact bytes, ids replaced by their labels.
fn family(a: &AssetServer, source: &str) -> BTreeMap<String, (String, Blake3)> {
    let sc = a.sidecar(&p(source)).expect("imported");
    let rec = sc.import.as_ref().expect("has a record");
    let labels: HashMap<AssetId, String> = rec
        .artefacts
        .iter()
        .map(|r| (r.id(), r.label().to_string()))
        .collect();
    let canon = |id: AssetId| {
        let l = labels
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("foreign {id}"));
        AssetId(u128::from_le_bytes(
            Blake3::of(l.as_bytes()).0[..16]
                .try_into()
                .expect("16 bytes"),
        ))
    };
    let mut out = BTreeMap::new();
    for r in &rec.artefacts {
        let bytes = a.vfs().blob_get(r.blob().expect("blob")).expect("stored");
        let norm: Bytes = match r.kind() {
            "mesh" => {
                let mut m = MeshAsset::decode(&bytes).expect("mesh");
                for pr in &mut m.primitives {
                    pr.material = pr.material.map(canon);
                }
                m.encode().expect("encodes")
            }
            "material" => {
                let mut m = MaterialAsset::decode(&bytes).expect("material");
                for t in [
                    &mut m.base_color_texture,
                    &mut m.metallic_roughness_texture,
                    &mut m.normal_texture,
                    &mut m.occlusion_texture,
                    &mut m.emissive_texture,
                ] {
                    *t = t.map(canon);
                }
                m.encode().expect("encodes")
            }
            "scene" => {
                let mut s = SceneAsset::decode(&bytes).expect("scene");
                for n in &mut s.nodes {
                    n.mesh = n.mesh.map(canon);
                }
                s.encode().expect("encodes")
            }
            _ => bytes,
        };
        out.insert(
            r.label().to_string(),
            (r.kind().to_string(), Blake3::of(&norm)),
        );
    }
    out
}

fn diff(
    a: &BTreeMap<String, (String, Blake3)>,
    b: &BTreeMap<String, (String, Blake3)>,
) -> Result<(), String> {
    for (l, v) in a {
        match b.get(l) {
            None => return Err(format!("{l:?} is missing after the round trip")),
            Some(w) if w != v => {
                return Err(format!("{l:?} ({}) differs after the round trip", v.0));
            }
            Some(_) => {}
        }
    }
    if let Some(extra) = b.keys().find(|k| !a.contains_key(*k)) {
        return Err(format!("{extra:?} appeared in the round trip"));
    }
    Ok(())
}

/// Import `models/scene.gltf`, export it, import the `.glb`, compare the families.
fn round_trip(a: &mut AssetServer) -> Result<(), String> {
    let id = common::import(a, "models/scene.gltf");
    let files = a.export(id, None, "exported").map_err(|e| e.to_string())?;
    for (name, bytes) in files {
        common::write(a, &format!("export/{name}"), bytes);
    }
    common::import(a, "export/exported.glb");
    diff(
        &family(a, "models/scene.gltf"),
        &family(a, "export/exported.glb"),
    )
}

fn server() -> AssetServer {
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "models", &spec());
    open(Box::new(s))
}

#[test]
fn imported_assets_match_what_the_file_says() {
    let mut a = server();
    let id = common::import(&mut a, "models/scene.gltf");
    let scene = a.load::<SceneAsset>(id).wait(WAIT).expect("scene loads");
    assert_eq!(scene.name, "scene");
    assert_eq!(scene.nodes.len(), 6);
    assert_eq!(scene.nodes[1].name, "Object_1");
    assert_eq!(scene.nodes[1].translation, [1.5, 0.0, 0.0]);
    assert_eq!(scene.nodes[1].rotation, [0.0, 0.0, 0.0, 1.0]);

    let mesh_id = scene.nodes[2].mesh.expect("node 2 draws a mesh");
    assert_eq!(mesh_id, id.child("mesh:Mesh_2"), "labels use unique names");
    let mesh = a.load::<MeshAsset>(mesh_id).wait(WAIT).expect("mesh loads");
    let prim = &mesh.primitives[0];
    assert_eq!(prim.vertex_count, 16); // (3+1)^2
    assert_eq!(prim.index_list().len(), 3 * 3 * 6);
    assert!(prim.attribute(Semantic::Normal).is_some());
    assert!(prim.attribute(Semantic::TexCoord(0)).is_some());
    assert_eq!(prim.positions()[0][0], -0.5);
    assert_eq!(prim.bounds.min[0], -0.5);
    assert_eq!(prim.bounds.max[2], 0.5);
    assert_eq!(prim.material, Some(id.child("material:Material_2")));

    let mat = a
        .load::<MaterialAsset>(id.child("material:Material_1"))
        .wait(WAIT)
        .expect("material loads");
    assert_eq!(mat.base_color, [1.0, 1.0, 0.25, 1.0]);
    assert_eq!(mat.roughness, 0.5);
    assert_eq!(mat.base_color_texture, Some(id.child("texture:Checker")));
    assert_eq!(mat.normal_texture, Some(id.child("texture:Normal@linear")));

    let tex = a
        .load::<TextureAsset>(id.child("texture:Checker"))
        .wait(WAIT)
        .expect("texture loads");
    assert_eq!((tex.width, tex.height, tex.srgb), (64, 64, true));
    assert_eq!(tex.levels.len(), 7, "a full mip chain 64..1");
    assert_eq!(&tex.levels[0][..4], &[230, 60, 40, 255]);
    let nrm = a
        .load::<TextureAsset>(id.child("texture:Normal@linear"))
        .wait(WAIT)
        .expect("normal map loads");
    assert!(!nrm.srgb, "a normal map is linear data");

    // Every dependency is recorded, exactly spelled.
    let deps: Vec<String> = a
        .sidecar(&p("models/scene.gltf"))
        .expect("sc")
        .import
        .as_ref()
        .expect("rec")
        .deps
        .iter()
        .map(|d| d.0.clone())
        .collect();
    assert_eq!(
        deps,
        [
            "models/scene.bin",
            "models/textures/checker.png",
            "models/textures/normal.png"
        ]
    );
    // A typed handle of the wrong type fails, it does not panic or lie.
    assert!(a.load::<MeshAsset>(id).wait(WAIT).is_err());
}

#[test]
fn export_then_import_gives_the_same_assets() {
    let mut a = server();
    round_trip(&mut a).expect("the round trip is exact");
    // And it is really a GLB with a binary chunk.
    let glb = a.vfs().read(&p("export/exported.glb")).expect("written");
    assert_eq!(&glb[..4], b"glTF");
}

#[test]
fn a_data_uri_buffer_imports_to_the_same_assets() {
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "models", &spec());
    let bin = s.read(&p("models/scene.bin")).expect("bin");
    let text =
        String::from_utf8(s.read(&p("models/scene.gltf")).expect("gltf").to_vec()).expect("utf8");
    let uri = format!(
        "data:application/octet-stream;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bin)
    );
    s.write(
        &p("models/inline.gltf"),
        Bytes::from(text.replace("\"scene.bin\"", &format!("\"{uri}\""))),
    )
    .expect("write");
    let mut a = open(Box::new(s));
    common::import(&mut a, "models/scene.gltf");
    common::import(&mut a, "models/inline.gltf");
    let (x, y) = (
        family(&a, "models/scene.gltf"),
        family(&a, "models/inline.gltf"),
    );
    let strip = |m: &BTreeMap<String, (String, Blake3)>| {
        m.iter()
            .filter(|(l, _)| !l.is_empty())
            .map(|(l, v)| (l.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    // Same meshes, materials and textures; the scene differs only by its name.
    diff(&strip(&x), &strip(&y)).expect("identical assets");
    let deps = &a
        .sidecar(&p("models/inline.gltf"))
        .expect("sc")
        .import
        .as_ref()
        .expect("r")
        .deps;
    assert!(
        deps.iter().all(|d| !d.0.ends_with(".bin")),
        "no external buffer read"
    );
}

#[test]
fn malformed_files_fail_with_a_code_not_a_panic() {
    let mut s = MemoryStore::new("ada");
    s.write(&p("bad/broken.gltf"), Bytes::from_static(b"{ not json"))
        .expect("w");
    s.write(
        &p("bad/short.gltf"),
        Bytes::from_static(
            br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"x.bin","byteLength":64}]}"#,
        ),
    )
    .expect("w");
    s.write(&p("bad/x.bin"), Bytes::from_static(&[0u8; 8]))
        .expect("w");
    s.write(
        &p("bad/image.png"),
        Bytes::from_static(b"\x89PNG\r\n\x1a\nnope"),
    )
    .expect("w");
    let mut a = open(Box::new(s));
    for f in ["bad/broken.gltf", "bad/short.gltf", "bad/image.png"] {
        let mut ev = Vec::new();
        let e = a.import(&p(f), Settings::new(), &mut ev).expect_err(f);
        assert_eq!(e.code().as_str(), "ASSET-0005", "{f}: {e}");
        assert!(
            a.id_of(&p(f)).is_none(),
            "{f}: a failed first import registers nothing"
        );
    }
}

// ---- positive control ----------------------------------------------------------------

/// Exports through an inner exporter whose view of every mesh has lost its normals.
struct DropNormals(Arc<dyn Exporter>);

struct Lossy<'a>(&'a dyn ExportSource);

impl ExportSource for Lossy<'_> {
    fn artefact(&self, id: AssetId) -> Result<ExportArtefact, forge_asset::AssetError> {
        let mut a = self.0.artefact(id)?;
        if a.kind == "mesh"
            && let Ok(mut m) = MeshAsset::decode(&a.bytes)
        {
            for p in &mut m.primitives {
                p.attributes.retain(|x| x.semantic != Semantic::Normal);
            }
            a.bytes = m.encode().expect("encodes");
        }
        Ok(a)
    }
}

impl Exporter for DropNormals {
    fn kinds(&self) -> &[&str] {
        self.0.kinds()
    }
    fn export(
        &self,
        src: &dyn ExportSource,
        root: AssetId,
        name: &str,
    ) -> Result<Vec<(String, Bytes)>, forge_asset::AssetError> {
        self.0.export(&Lossy(src), root, name)
    }
}

#[test]
fn positive_control_a_lossy_exporter_fails_the_round_trip() {
    let mut x = forge_importers::asset_extensions().expect("extensions");
    let mutant = PluginId::new("test.mutant").expect("id");
    x.registry_mut::<ExporterPoint>()
        .expect("defined")
        .chain(
            &mutant,
            &ItemId::of::<ExporterPoint>("gltf").expect("item"),
            |inner| Arc::new(DropNormals(inner)) as Arc<dyn Exporter>,
        )
        .expect("chains");
    let mut s = MemoryStore::new("ada");
    put_scene(&mut s, "models", &spec());
    let (mut a, _) = AssetServer::open(Box::new(s), &x).expect("opens");
    let err = round_trip(&mut a).expect_err("the lossy exporter must be caught");
    assert!(err.contains("mesh:"), "names the damaged mesh: {err}");
}
