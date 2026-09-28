//! The glTF 2.0 importer (`.gltf` JSON with external or data-URI buffers, and `.glb`).
//!
//! One source becomes a family of assets, each with a stable sub-asset id:
//!
//! | label | kind | what |
//! |---|---|---|
//! | `""` (main) | `scene` | the default scene's node hierarchy, meshes by id |
//! | `mesh:<name>` / `mesh#<i>` | `mesh` | every mesh, all primitives, GPU-layout streams |
//! | `material:<name>` / `material#<i>` | `material` | metallic-roughness materials |
//! | `texture:<name>` / `texture#<i>` (`@linear`) | `texture` | every image, as KTX2 with mips |
//!
//! Labels use the glTF name when it is present and unique in its category, so reordering
//! meshes in the authoring tool keeps every id; otherwise the index. An image used both as
//! colour and as data (a normal map) yields an sRGB and a `@linear` texture.
//!
//! **Every external file is read through [`ImportCx::dependency`]**: resolved exactly
//! (M0-17 — a wrong-case `uri` fails on every platform) and recorded, so editing the `.bin`
//! or a texture reimports the glTF (hot reload). No `std::fs` (I17): `gltf`'s own `import`
//! feature is off for exactly this reason.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use bytes::Bytes;
use gltf::mesh::Mode;

use crate::image_import::decode_image;
use forge_asset::types::{
    Aabb, AlphaMode, MaterialAsset, MeshAsset, Primitive, SceneAsset, SceneNode, Semantic,
    Topology, VertexAttribute, VertexFormat,
};
use forge_asset::{AssetError, AssetId};
use forge_asset::{ImportCx, Importer};

/// The glTF 2.0 importer.
pub struct GltfImporter;

/// Stable labels for one category: the name if present and unique, else `#index`.
fn labels<'a>(prefix: &str, names: impl Iterator<Item = Option<&'a str>>) -> Vec<String> {
    let names: Vec<Option<&str>> = names.collect();
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for n in names.iter().flatten() {
        *count.entry(n).or_default() += 1;
    }
    names
        .iter()
        .enumerate()
        .map(|(i, n)| match n {
            Some(n) if !n.is_empty() && count.get(n) == Some(&1) => format!("{prefix}:{n}"),
            _ => format!("{prefix}#{i}"),
        })
        .collect()
}

fn display(name: Option<&str>, fallback: &str, i: usize) -> String {
    match name {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => format!("{fallback} {i}"),
    }
}

fn data_uri(uri: &str) -> Option<Result<Vec<u8>, String>> {
    let rest = uri.strip_prefix("data:")?;
    Some(match rest.split_once(";base64,") {
        Some((_, b64)) => base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("bad base64 data URI: {e}")),
        None => Err("only base64 data URIs are supported".into()),
    })
}

fn f32_bytes<const N: usize>(it: impl Iterator<Item = [f32; N]>) -> (Vec<u8>, u32) {
    let mut out = Vec::new();
    let mut n = 0u32;
    for v in it {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
        n += 1;
    }
    (out, n)
}

impl GltfImporter {
    fn buffers(cx: &mut ImportCx<'_>, doc: &gltf::Gltf) -> Result<Vec<Bytes>, AssetError> {
        let mut out = Vec::new();
        for b in doc.buffers() {
            let data = match b.source() {
                gltf::buffer::Source::Bin => doc
                    .blob
                    .clone()
                    .map(Bytes::from)
                    .ok_or_else(|| cx.fail("buffer refers to a missing GLB binary chunk"))?,
                gltf::buffer::Source::Uri(uri) => match data_uri(uri) {
                    Some(r) => Bytes::from(r.map_err(|e| cx.fail(e))?),
                    None => cx.dependency(uri)?,
                },
            };
            if data.len() < b.length() {
                return Err(cx.fail(format!(
                    "buffer {} holds {} bytes; the file declares {}",
                    b.index(),
                    data.len(),
                    b.length()
                )));
            }
            out.push(data);
        }
        Ok(out)
    }

    fn image_bytes(
        cx: &mut ImportCx<'_>,
        img: &gltf::Image<'_>,
        buffers: &[Bytes],
    ) -> Result<Bytes, AssetError> {
        match img.source() {
            gltf::image::Source::View { view, .. } => {
                let b = buffers
                    .get(view.buffer().index())
                    .ok_or_else(|| cx.fail("image view names a missing buffer"))?;
                let (s, e) = (view.offset(), view.offset() + view.length());
                if e > b.len() {
                    return Err(cx.fail(format!("image {} runs past its buffer", img.index())));
                }
                Ok(b.slice(s..e))
            }
            gltf::image::Source::Uri { uri, .. } => match data_uri(uri) {
                Some(r) => Ok(Bytes::from(r.map_err(|e| cx.fail(e))?)),
                None => cx.dependency(uri),
            },
        }
    }

    fn primitive(
        cx: &ImportCx<'_>,
        p: &gltf::Primitive<'_>,
        buffers: &[Bytes],
        materials: &[AssetId],
        mesh: usize,
    ) -> Result<Primitive, AssetError> {
        let r = p.reader(|b| buffers.get(b.index()).map(|d| &d[..]));
        let where_ = |what: &str| format!("mesh {mesh} primitive {}: {what}", p.index());
        let (pos, n) = f32_bytes(
            r.read_positions()
                .ok_or_else(|| cx.fail(where_("no readable POSITION")))?,
        );
        let mut bounds = Aabb::EMPTY;
        for c in pos.as_chunks::<12>().0 {
            let v = [0, 4, 8]
                .map(|o| f64::from(f32::from_le_bytes([c[o], c[o + 1], c[o + 2], c[o + 3]])));
            if v.iter().any(|x| !x.is_finite()) {
                return Err(cx.fail(where_("a non-finite position")));
            }
            bounds.grow(v);
        }
        let mut attributes = vec![VertexAttribute {
            semantic: Semantic::Position,
            format: VertexFormat::Float32x3,
            data: Bytes::from(pos),
        }];
        let mut push = |semantic, format, (data, count): (Vec<u8>, u32)| {
            if count != n {
                return Err(cx.fail(where_(&format!(
                    "{semantic:?} has {count} elements, POSITION has {n}"
                ))));
            }
            attributes.push(VertexAttribute {
                semantic,
                format,
                data: Bytes::from(data),
            });
            Ok(())
        };
        if let Some(it) = r.read_normals() {
            push(Semantic::Normal, VertexFormat::Float32x3, f32_bytes(it))?;
        }
        if let Some(it) = r.read_tangents() {
            push(Semantic::Tangent, VertexFormat::Float32x4, f32_bytes(it))?;
        }
        for set in 0..8u8 {
            let Some(it) = r.read_tex_coords(u32::from(set)) else {
                break;
            };
            push(
                Semantic::TexCoord(set),
                VertexFormat::Float32x2,
                f32_bytes(it.into_f32()),
            )?;
        }
        for set in 0..8u8 {
            let Some(it) = r.read_colors(u32::from(set)) else {
                break;
            };
            push(
                Semantic::Color(set),
                VertexFormat::Float32x4,
                f32_bytes(it.into_rgba_f32()),
            )?;
        }
        for set in 0..8u8 {
            let Some(it) = r.read_joints(u32::from(set)) else {
                break;
            };
            let mut data = Vec::new();
            let mut count = 0;
            for j in it.into_u16() {
                for x in j {
                    data.extend_from_slice(&x.to_le_bytes());
                }
                count += 1;
            }
            push(Semantic::Joints(set), VertexFormat::Uint16x4, (data, count))?;
        }
        for set in 0..8u8 {
            let Some(it) = r.read_weights(u32::from(set)) else {
                break;
            };
            push(
                Semantic::Weights(set),
                VertexFormat::Float32x4,
                f32_bytes(it.into_f32()),
            )?;
        }
        attributes.sort_by_key(|a| a.semantic);

        let mut idx: Option<Vec<u32>> = r.read_indices().map(|i| i.into_u32().collect());
        if let Some(i) = &idx
            && let Some(bad) = i.iter().find(|&&v| v >= n)
        {
            return Err(cx.fail(where_(&format!("index {bad} >= vertex count {n}"))));
        }
        let topology = match p.mode() {
            Mode::Points => Topology::Points,
            Mode::Lines => Topology::Lines,
            Mode::LineStrip => Topology::LineStrip,
            Mode::Triangles => Topology::Triangles,
            Mode::TriangleStrip => Topology::TriangleStrip,
            // Fans and loops have no GPU topology: rewrite them as lists / strips.
            Mode::TriangleFan => {
                let src = idx.take().unwrap_or_else(|| (0..n).collect());
                let mut tri = Vec::with_capacity(src.len().saturating_sub(2) * 3);
                for k in 1..src.len().saturating_sub(1) {
                    tri.extend_from_slice(&[src[0], src[k], src[k + 1]]);
                }
                idx = Some(tri);
                Topology::Triangles
            }
            Mode::LineLoop => {
                let mut src = idx.take().unwrap_or_else(|| (0..n).collect());
                if let Some(&first) = src.first() {
                    src.push(first);
                }
                idx = Some(src);
                Topology::LineStrip
            }
        };
        let indices =
            idx.map(|i| Bytes::from(i.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()));
        let material = match p.material().index() {
            Some(m) => Some(
                *materials
                    .get(m)
                    .ok_or_else(|| cx.fail(where_("names a missing material")))?,
            ),
            None => None,
        };
        Ok(Primitive {
            topology,
            vertex_count: n,
            attributes,
            indices,
            material,
            bounds,
        })
    }
}

impl Importer for GltfImporter {
    fn version(&self) -> u32 {
        1
    }

    fn extensions(&self) -> &[&str] {
        &["gltf", "glb"]
    }

    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        let doc = gltf::Gltf::from_slice(cx.source()).map_err(|e| cx.fail(e))?;
        let buffers = Self::buffers(cx, &doc)?;
        let mips = cx.flag("mips", true);

        // Which images are used as colour (sRGB) and which as data (linear).
        let mut usage: BTreeMap<usize, BTreeSet<bool>> = BTreeMap::new();
        let tex_image = |t: gltf::Texture<'_>| t.source().index();
        for m in doc.materials() {
            let pbr = m.pbr_metallic_roughness();
            if let Some(t) = pbr.base_color_texture() {
                usage
                    .entry(tex_image(t.texture()))
                    .or_default()
                    .insert(true);
            }
            if let Some(t) = m.emissive_texture() {
                usage
                    .entry(tex_image(t.texture()))
                    .or_default()
                    .insert(true);
            }
            if let Some(t) = pbr.metallic_roughness_texture() {
                usage
                    .entry(tex_image(t.texture()))
                    .or_default()
                    .insert(false);
            }
            if let Some(t) = m.normal_texture() {
                usage
                    .entry(tex_image(t.texture()))
                    .or_default()
                    .insert(false);
            }
            if let Some(t) = m.occlusion_texture() {
                usage
                    .entry(tex_image(t.texture()))
                    .or_default()
                    .insert(false);
            }
        }

        // Textures: one artefact per (image, colour space) used.
        let img_labels = labels("texture", doc.images().map(|i| i.name()));
        let mut tex_ids: BTreeMap<(usize, bool), AssetId> = BTreeMap::new();
        for img in doc.images() {
            let i = img.index();
            let spaces = usage.get(&i).cloned().unwrap_or_else(|| [true].into());
            let bytes = Self::image_bytes(cx, &img, &buffers)?;
            for srgb in spaces {
                let tex = decode_image(&bytes, srgb, mips)
                    .map_err(|e| cx.fail(format!("image {i}: {e}")))?;
                let label = if srgb {
                    img_labels[i].clone()
                } else {
                    format!("{}@linear", img_labels[i])
                };
                let name = display(img.name(), "Texture", i);
                let id = cx.emit_value(&label, &name, &tex)?;
                tex_ids.insert((i, srgb), id);
            }
        }
        let tex =
            |t: gltf::Texture<'_>, srgb: bool| tex_ids.get(&(t.source().index(), srgb)).copied();

        // Materials.
        let mat_labels = labels("material", doc.materials().map(|m| m.name()));
        let mut materials = Vec::new();
        for m in doc.materials() {
            let Some(i) = m.index() else { continue };
            let pbr = m.pbr_metallic_roughness();
            let mat = MaterialAsset {
                name: display(m.name(), "Material", i),
                base_color: pbr.base_color_factor().map(f64::from),
                base_color_texture: pbr
                    .base_color_texture()
                    .and_then(|t| tex(t.texture(), true)),
                metallic: f64::from(pbr.metallic_factor()),
                roughness: f64::from(pbr.roughness_factor()),
                metallic_roughness_texture: pbr
                    .metallic_roughness_texture()
                    .and_then(|t| tex(t.texture(), false)),
                normal_texture: m.normal_texture().and_then(|t| tex(t.texture(), false)),
                normal_scale: m.normal_texture().map_or(1.0, |t| f64::from(t.scale())),
                occlusion_texture: m.occlusion_texture().and_then(|t| tex(t.texture(), false)),
                emissive: m.emissive_factor().map(f64::from),
                emissive_texture: m.emissive_texture().and_then(|t| tex(t.texture(), true)),
                alpha_mode: match m.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                    gltf::material::AlphaMode::Mask => {
                        AlphaMode::Mask(f64::from(m.alpha_cutoff().unwrap_or(0.5)))
                    }
                    gltf::material::AlphaMode::Blend => AlphaMode::Blend,
                },
                double_sided: m.double_sided(),
            };
            let name = mat.name.clone();
            materials.push(cx.emit_value(&mat_labels[i], &name, &mat)?);
        }

        // Meshes.
        let mesh_labels = labels("mesh", doc.meshes().map(|m| m.name()));
        let mut mesh_ids = Vec::new();
        for m in doc.meshes() {
            let i = m.index();
            let mut prims = Vec::new();
            for p in m.primitives() {
                prims.push(Self::primitive(cx, &p, &buffers, &materials, i)?);
            }
            let mesh = MeshAsset {
                name: display(m.name(), "Mesh", i),
                primitives: prims,
            };
            let name = mesh.name.clone();
            mesh_ids.push(cx.emit_value(&mesh_labels[i], &name, &mesh)?);
        }

        // The scene: the default one (else the first; else every root node), parents first.
        let scene = doc.default_scene().or_else(|| doc.scenes().next());
        let roots: Vec<gltf::Node<'_>> = match &scene {
            Some(s) => s.nodes().collect(),
            None => {
                let children: BTreeSet<usize> = doc
                    .nodes()
                    .flat_map(|n| n.children().map(|c| c.index()).collect::<Vec<_>>())
                    .collect();
                doc.nodes()
                    .filter(|n| !children.contains(&n.index()))
                    .collect()
            }
        };
        let mut nodes = Vec::new();
        let mut stack: Vec<(gltf::Node<'_>, Option<u32>)> =
            roots.into_iter().rev().map(|n| (n, None)).collect();
        let mut visited = BTreeSet::new();
        while let Some((n, parent)) = stack.pop() {
            if !visited.insert(n.index()) {
                return Err(cx.fail(format!("node {} has two parents", n.index())));
            }
            let (t, r, s) = n.transform().decomposed();
            let widen3 = |v: [f32; 3]| v.map(f64::from);
            let me = nodes.len() as u32;
            nodes.push(SceneNode {
                name: display(n.name(), "Node", n.index()),
                parent,
                translation: widen3(t),
                rotation: r.map(f64::from),
                scale: widen3(s),
                mesh: n.mesh().and_then(|m| mesh_ids.get(m.index()).copied()),
            });
            let kids: Vec<_> = n.children().collect();
            for c in kids.into_iter().rev() {
                stack.push((c, Some(me)));
            }
        }
        if nodes.iter().any(|n| {
            n.translation
                .iter()
                .chain(&n.rotation)
                .chain(&n.scale)
                .any(|x| !x.is_finite())
        }) {
            return Err(cx.fail("a node transform is not finite"));
        }
        let stem = cx.path().file_name();
        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s).to_string();
        let scene_asset = SceneAsset {
            name: scene
                .and_then(|s| s.name().map(str::to_string))
                .filter(|s| !s.is_empty())
                .unwrap_or(stem),
            nodes,
        };
        let name = scene_asset.name.clone();
        cx.emit_value("", &name, &scene_asset)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_prefer_unique_names() {
        let l = labels(
            "mesh",
            [Some("Hull"), Some("Wing"), Some("Wing"), None, Some("")].into_iter(),
        );
        assert_eq!(l, ["mesh:Hull", "mesh#1", "mesh#2", "mesh#3", "mesh#4"]);
    }

    #[test]
    fn data_uris_decode() {
        assert_eq!(
            data_uri("data:application/octet-stream;base64,AAEC"),
            Some(Ok(vec![0, 1, 2]))
        );
        assert!(matches!(data_uri("data:text/plain,hi"), Some(Err(_))));
        assert_eq!(data_uri("model.bin"), None);
    }
}
