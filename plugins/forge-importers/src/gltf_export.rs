//! The glTF 2.0 exporter: a `scene` asset (and every mesh, material and texture it reaches)
//! back to one self-contained `.glb`.
//!
//! It exists so the importer can be tested by **round trip** (import -> export -> import
//! gives the same assets, `tests/test_gltf_roundtrip.rs`) and so a project's assets can
//! leave the engine in an open format. Vertex streams are written byte for byte; `f64`
//! factors that came from glTF's `f32` narrow back exactly; textures are written as PNG
//! (level 0; mips are regenerated deterministically on import).

use std::collections::BTreeMap;

use bytes::Bytes;
use forge_store::Blake3;
use serde_json::{Value as J, json};

use forge_asset::types::{
    AlphaMode, AssetValue, KIND_SCENE, MaterialAsset, MeshAsset, SceneAsset, Semantic,
    TextureAsset, Topology, VertexFormat,
};
use forge_asset::{AssetError, AssetId};
use forge_asset::{ExportSource, Exporter};

/// The glTF (`.glb`) exporter.
pub struct GltfExporter;

#[derive(Default)]
struct Out {
    bin: Vec<u8>,
    views: Vec<J>,
    accessors: Vec<J>,
    images: Vec<J>,
    textures: Vec<J>,
    materials: Vec<J>,
    meshes: Vec<J>,
    mesh_ix: BTreeMap<AssetId, usize>,
    mat_ix: BTreeMap<AssetId, usize>,
    tex_ix: BTreeMap<AssetId, usize>,
    image_ix: BTreeMap<Blake3, usize>,
}

impl Out {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }
}

fn narrow(x: f64) -> J {
    // Values here came from glTF f32s (or are meant to be stored as f32): write the f32.
    json!(f64::from(x as f32))
}

fn fail(id: AssetId, why: impl std::fmt::Display) -> AssetError {
    AssetError::Export {
        id: id.to_string(),
        why: why.to_string(),
    }
}

impl GltfExporter {
    fn texture(
        o: &mut Out,
        src: &dyn ExportSource,
        id: Option<AssetId>,
    ) -> Result<Option<usize>, AssetError> {
        let Some(id) = id else { return Ok(None) };
        if let Some(&i) = o.tex_ix.get(&id) {
            return Ok(Some(i));
        }
        let a = src.artefact(id)?;
        let t = TextureAsset::decode(&a.bytes).map_err(|e| fail(id, e))?;
        let l0 = t.levels.first().ok_or_else(|| fail(id, "no levels"))?;
        let key = {
            let mut h = blake3::Hasher::new();
            h.update(&t.width.to_le_bytes());
            h.update(&t.height.to_le_bytes());
            h.update(l0);
            Blake3(*h.finalize().as_bytes())
        };
        let image = match o.image_ix.get(&key) {
            Some(&i) => i,
            None => {
                let mut png = Vec::new();
                image::ImageEncoder::write_image(
                    image::codecs::png::PngEncoder::new(&mut png),
                    l0,
                    t.width,
                    t.height,
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|e| fail(id, e))?;
                let view = o.view(&png, None);
                let name = a.name.trim_end_matches(".png").to_string();
                o.images
                    .push(json!({ "name": name, "bufferView": view, "mimeType": "image/png" }));
                o.image_ix.insert(key, o.images.len() - 1);
                o.images.len() - 1
            }
        };
        o.textures.push(json!({ "source": image }));
        let ix = o.textures.len() - 1;
        o.tex_ix.insert(id, ix);
        Ok(Some(ix))
    }

    fn material(o: &mut Out, src: &dyn ExportSource, id: AssetId) -> Result<usize, AssetError> {
        if let Some(&i) = o.mat_ix.get(&id) {
            return Ok(i);
        }
        let a = src.artefact(id)?;
        let m = MaterialAsset::decode(&a.bytes).map_err(|e| fail(id, e))?;
        let mut pbr = json!({
            "baseColorFactor": m.base_color.map(narrow),
            "metallicFactor": narrow(m.metallic),
            "roughnessFactor": narrow(m.roughness),
        });
        if let Some(t) = Self::texture(o, src, m.base_color_texture)? {
            pbr["baseColorTexture"] = json!({ "index": t });
        }
        if let Some(t) = Self::texture(o, src, m.metallic_roughness_texture)? {
            pbr["metallicRoughnessTexture"] = json!({ "index": t });
        }
        let mut j = json!({
            "name": m.name,
            "pbrMetallicRoughness": pbr,
            "emissiveFactor": m.emissive.map(narrow),
            "doubleSided": m.double_sided,
        });
        if let Some(t) = Self::texture(o, src, m.normal_texture)? {
            j["normalTexture"] = json!({ "index": t, "scale": narrow(m.normal_scale) });
        }
        if let Some(t) = Self::texture(o, src, m.occlusion_texture)? {
            j["occlusionTexture"] = json!({ "index": t });
        }
        if let Some(t) = Self::texture(o, src, m.emissive_texture)? {
            j["emissiveTexture"] = json!({ "index": t });
        }
        match m.alpha_mode {
            AlphaMode::Opaque => {}
            AlphaMode::Mask(c) => {
                j["alphaMode"] = json!("MASK");
                j["alphaCutoff"] = narrow(c);
            }
            AlphaMode::Blend => j["alphaMode"] = json!("BLEND"),
        }
        o.materials.push(j);
        let ix = o.materials.len() - 1;
        o.mat_ix.insert(id, ix);
        Ok(ix)
    }

    fn mesh(o: &mut Out, src: &dyn ExportSource, id: AssetId) -> Result<usize, AssetError> {
        if let Some(&i) = o.mesh_ix.get(&id) {
            return Ok(i);
        }
        let a = src.artefact(id)?;
        let m = MeshAsset::decode(&a.bytes).map_err(|e| fail(id, e))?;
        let mut prims = Vec::new();
        for p in &m.primitives {
            let mut attrs = serde_json::Map::new();
            for at in &p.attributes {
                let name = match at.semantic {
                    Semantic::Position => "POSITION".to_string(),
                    Semantic::Normal => "NORMAL".to_string(),
                    Semantic::Tangent => "TANGENT".to_string(),
                    Semantic::TexCoord(n) => format!("TEXCOORD_{n}"),
                    Semantic::Color(n) => format!("COLOR_{n}"),
                    Semantic::Joints(n) => format!("JOINTS_{n}"),
                    Semantic::Weights(n) => format!("WEIGHTS_{n}"),
                };
                let (ctype, ty) = match at.format {
                    VertexFormat::Float32x2 => (5126, "VEC2"),
                    VertexFormat::Float32x3 => (5126, "VEC3"),
                    VertexFormat::Float32x4 => (5126, "VEC4"),
                    VertexFormat::Uint16x4 => (5123, "VEC4"),
                };
                let view = o.view(&at.data, Some(34962));
                let mut acc = json!({
                    "bufferView": view, "componentType": ctype, "count": p.vertex_count, "type": ty,
                });
                if at.semantic == Semantic::Position {
                    acc["min"] = json!(p.bounds.min.map(narrow));
                    acc["max"] = json!(p.bounds.max.map(narrow));
                }
                o.accessors.push(acc);
                attrs.insert(name, json!(o.accessors.len() - 1));
            }
            let mode = match p.topology {
                Topology::Points => 0,
                Topology::Lines => 1,
                Topology::LineStrip => 3,
                Topology::Triangles => 4,
                Topology::TriangleStrip => 5,
            };
            let mut pj = json!({ "attributes": attrs, "mode": mode });
            if let Some(ix) = &p.indices {
                let view = o.view(ix, Some(34963));
                o.accessors.push(json!({
                    "bufferView": view, "componentType": 5125, "count": ix.len() / 4, "type": "SCALAR",
                }));
                pj["indices"] = json!(o.accessors.len() - 1);
            }
            if let Some(mat) = p.material {
                pj["material"] = json!(Self::material(o, src, mat)?);
            }
            prims.push(pj);
        }
        o.meshes
            .push(json!({ "name": m.name, "primitives": prims }));
        let ix = o.meshes.len() - 1;
        o.mesh_ix.insert(id, ix);
        Ok(ix)
    }
}

/// Wrap JSON and a binary chunk as GLB.
fn glb(json_text: &[u8], bin: &[u8]) -> Vec<u8> {
    let pad = |n: usize| (4 - n % 4) % 4;
    let (jp, bp) = (pad(json_text.len()), pad(bin.len()));
    let total = 12 + 8 + json_text.len() + jp + 8 + bin.len() + bp;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&((json_text.len() + jp) as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(json_text);
    out.extend(std::iter::repeat_n(b' ', jp));
    out.extend_from_slice(&((bin.len() + bp) as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(bin);
    out.extend(std::iter::repeat_n(0u8, bp));
    out
}

impl Exporter for GltfExporter {
    fn kinds(&self) -> &[&str] {
        &[KIND_SCENE]
    }

    fn export(
        &self,
        src: &dyn ExportSource,
        root: AssetId,
        name: &str,
    ) -> Result<Vec<(String, Bytes)>, AssetError> {
        let a = src.artefact(root)?;
        if a.kind != KIND_SCENE {
            return Err(fail(root, format!("a {:?} is not a scene", a.kind)));
        }
        let scene = SceneAsset::decode(&a.bytes).map_err(|e| fail(root, e))?;
        let mut o = Out::default();
        let mut nodes = Vec::with_capacity(scene.nodes.len());
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); scene.nodes.len()];
        let mut roots = Vec::new();
        for (i, n) in scene.nodes.iter().enumerate() {
            match n.parent {
                Some(p) => children[p as usize].push(i),
                None => roots.push(i),
            }
        }
        for (i, n) in scene.nodes.iter().enumerate() {
            let mut j = json!({
                "name": n.name,
                "translation": n.translation.map(narrow),
                "rotation": n.rotation.map(narrow),
                "scale": n.scale.map(narrow),
            });
            if !children[i].is_empty() {
                j["children"] = json!(children[i]);
            }
            if let Some(m) = n.mesh {
                j["mesh"] = json!(GltfExporter::mesh(&mut o, src, m)?);
            }
            nodes.push(j);
        }
        let mut doc = json!({
            "asset": { "version": "2.0", "generator": "Forge Engine forge-asset" },
            "scene": 0,
            "scenes": [ { "name": scene.name, "nodes": roots } ],
            "nodes": nodes,
        });
        for (key, v) in [
            ("meshes", &o.meshes),
            ("materials", &o.materials),
            ("textures", &o.textures),
            ("images", &o.images),
            ("accessors", &o.accessors),
            ("bufferViews", &o.views),
        ] {
            if !v.is_empty() {
                doc[key] = json!(v);
            }
        }
        if !o.bin.is_empty() {
            doc["buffers"] = json!([{ "byteLength": o.bin.len() }]);
        }
        let text = serde_json::to_vec(&doc).map_err(|e| fail(root, e))?;
        Ok(vec![(
            format!("{name}.glb"),
            Bytes::from(glb(&text, &o.bin)),
        )])
    }
}
