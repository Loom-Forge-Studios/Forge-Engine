//! The built-in asset types and their artefact formats.
//!
//! | kind | type | artefact bytes |
//! |---|---|---|
//! | `mesh` | [`MeshAsset`] | `FMSH` binary: GPU-layout vertex streams + `u32` indices, LE |
//! | `texture` | [`TextureAsset`] | a standard **KTX2** container (RGBA8, full mip chain) |
//! | `material` | [`MaterialAsset`] | RON (diffable) |
//! | `scene` | [`SceneAsset`] | RON (diffable): the node hierarchy, meshes by [`AssetId`] |
//!
//! Vertex streams are stored in the layout the GPU consumes (`Float32x3` positions, ...):
//! they are **data**, not math, and converting them would double their size and cost a pass
//! at load. Everything forge-asset computes (bounds, transforms, thumbnails) is `f64`; an
//! `f32` is only ever decoded from a stream and widened immediately (ADR 0015).

use std::any::Any;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::bin::{Reader, Writer};

/// The kind of mesh artefacts.
pub const KIND_MESH: &str = "mesh";
/// The kind of texture artefacts.
pub const KIND_TEXTURE: &str = "texture";
/// The kind of material artefacts.
pub const KIND_MATERIAL: &str = "material";
/// The kind of scene artefacts.
pub const KIND_SCENE: &str = "scene";

/// A type an artefact decodes to. Implemented by the built-in types; a plugin's own asset
/// type implements it and registers with [`crate::AssetTypeItem::of`].
pub trait AssetValue: Any + Send + Sync + Sized {
    /// Its kind (the `AssetType` registry key).
    const KIND: &'static str;
    /// Decode an artefact.
    fn decode(bytes: &Bytes) -> Result<Self, String>;
    /// Encode it as an artefact (deterministic bytes). A failure is an error the caller
    /// surfaces (an import fails with it), never an empty artefact.
    fn encode(&self) -> Result<Bytes, String>;
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    /// Minimum corner.
    pub min: [f64; 3],
    /// Maximum corner.
    pub max: [f64; 3],
}

impl Aabb {
    /// The empty box (grows from nothing).
    pub const EMPTY: Self = Self {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };

    /// Grow to contain `p`.
    pub fn grow(&mut self, p: [f64; 3]) {
        for (i, v) in p.into_iter().enumerate() {
            self.min[i] = self.min[i].min(v);
            self.max[i] = self.max[i].max(v);
        }
    }

    /// True if it contains no point.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        (0..3).any(|i| self.min[i] > self.max[i])
    }
}

/// What a vertex stream means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Semantic {
    /// Position.
    Position,
    /// Normal.
    Normal,
    /// Tangent (xyz + handedness w).
    Tangent,
    /// Texture coordinate set n.
    TexCoord(u8),
    /// Vertex colour set n.
    Color(u8),
    /// Skin joint indices set n.
    Joints(u8),
    /// Skin weights set n.
    Weights(u8),
}

impl Semantic {
    fn code(self) -> (u8, u8) {
        match self {
            Self::Position => (0, 0),
            Self::Normal => (1, 0),
            Self::Tangent => (2, 0),
            Self::TexCoord(n) => (3, n),
            Self::Color(n) => (4, n),
            Self::Joints(n) => (5, n),
            Self::Weights(n) => (6, n),
        }
    }
    fn from_code(c: u8, n: u8) -> Result<Self, String> {
        Ok(match c {
            0 => Self::Position,
            1 => Self::Normal,
            2 => Self::Tangent,
            3 => Self::TexCoord(n),
            4 => Self::Color(n),
            5 => Self::Joints(n),
            6 => Self::Weights(n),
            _ => return Err(format!("unknown vertex semantic {c}")),
        })
    }
}

/// A vertex stream's element layout (GPU vertex formats).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VertexFormat {
    /// Two LE `f32`.
    Float32x2,
    /// Three LE `f32`.
    Float32x3,
    /// Four LE `f32`.
    Float32x4,
    /// Four LE `u16`.
    Uint16x4,
}

impl VertexFormat {
    /// Bytes per element.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            Self::Float32x2 | Self::Uint16x4 => 8,
            Self::Float32x3 => 12,
            Self::Float32x4 => 16,
        }
    }
    fn code(self) -> u8 {
        match self {
            Self::Float32x2 => 0,
            Self::Float32x3 => 1,
            Self::Float32x4 => 2,
            Self::Uint16x4 => 3,
        }
    }
    fn from_code(c: u8) -> Result<Self, String> {
        Ok(match c {
            0 => Self::Float32x2,
            1 => Self::Float32x3,
            2 => Self::Float32x4,
            3 => Self::Uint16x4,
            _ => return Err(format!("unknown vertex format {c}")),
        })
    }
}

/// One vertex stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexAttribute {
    /// Its meaning.
    pub semantic: Semantic,
    /// Its layout.
    pub format: VertexFormat,
    /// `vertex_count * format.size()` bytes.
    pub data: Bytes,
}

/// Primitive topology.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Topology {
    /// Points.
    Points,
    /// Line list.
    Lines,
    /// Line strip.
    LineStrip,
    /// Triangle list.
    Triangles,
    /// Triangle strip.
    TriangleStrip,
}

impl Topology {
    fn code(self) -> u8 {
        match self {
            Self::Points => 0,
            Self::Lines => 1,
            Self::LineStrip => 2,
            Self::Triangles => 3,
            Self::TriangleStrip => 4,
        }
    }
    fn from_code(c: u8) -> Result<Self, String> {
        Ok(match c {
            0 => Self::Points,
            1 => Self::Lines,
            2 => Self::LineStrip,
            3 => Self::Triangles,
            4 => Self::TriangleStrip,
            _ => return Err(format!("unknown topology {c}")),
        })
    }
}

/// One draw's worth of a mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    /// Topology.
    pub topology: Topology,
    /// Vertices per stream.
    pub vertex_count: u32,
    /// Streams, sorted by semantic.
    pub attributes: Vec<VertexAttribute>,
    /// LE `u32` indices, if indexed.
    pub indices: Option<Bytes>,
    /// Its material.
    pub material: Option<AssetId>,
    /// Bounds of its positions.
    pub bounds: Aabb,
}

fn f32_at(b: &[u8], i: usize) -> f64 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[i..i + 4]);
    f64::from(f32::from_le_bytes(a))
}

impl Primitive {
    /// The stream with `semantic`.
    #[must_use]
    pub fn attribute(&self, semantic: Semantic) -> Option<&VertexAttribute> {
        self.attributes.iter().find(|a| a.semantic == semantic)
    }

    /// Positions, widened to `f64`.
    #[must_use]
    pub fn positions(&self) -> Vec<[f64; 3]> {
        match self.attribute(Semantic::Position) {
            Some(a) if a.format == VertexFormat::Float32x3 => a
                .data
                .as_chunks::<12>()
                .0
                .iter()
                .map(|c| [f32_at(c, 0), f32_at(c, 4), f32_at(c, 8)])
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Indices (or `0..vertex_count` for a non-indexed primitive).
    #[must_use]
    pub fn index_list(&self) -> Vec<u32> {
        match &self.indices {
            Some(b) => b
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            None => (0..self.vertex_count).collect(),
        }
    }
}

/// A mesh: one or more primitives.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshAsset {
    /// Display name.
    pub name: String,
    /// Its primitives.
    pub primitives: Vec<Primitive>,
}

impl MeshAsset {
    /// The union of the primitives' bounds.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in &self.primitives {
            if !p.bounds.is_empty() {
                b.grow(p.bounds.min);
                b.grow(p.bounds.max);
            }
        }
        b
    }
}

const MESH_MAGIC: &[u8; 4] = b"FMSH";
const MESH_VERSION: u32 = 1;

impl AssetValue for MeshAsset {
    const KIND: &'static str = KIND_MESH;

    fn encode(&self) -> Result<Bytes, String> {
        let mut w = Writer::with_magic(MESH_MAGIC, MESH_VERSION);
        w.str(&self.name);
        w.len(self.primitives.len());
        for p in &self.primitives {
            w.u8(p.topology.code());
            w.u32(p.vertex_count);
            w.len(p.attributes.len());
            for a in &p.attributes {
                let (c, n) = a.semantic.code();
                w.u8(c);
                w.u8(n);
                w.u8(a.format.code());
                w.bytes(&a.data);
            }
            match &p.indices {
                Some(i) => {
                    w.u8(1);
                    w.bytes(i);
                }
                None => w.u8(0),
            }
            match p.material {
                Some(m) => {
                    w.u8(1);
                    w.u128(m.0);
                }
                None => w.u8(0),
            }
            for v in p.bounds.min.iter().chain(&p.bounds.max) {
                w.f64(*v);
            }
        }
        Ok(w.finish())
    }

    fn decode(bytes: &Bytes) -> Result<Self, String> {
        let mut r = Reader::new(bytes, MESH_MAGIC, MESH_VERSION)?;
        let name = r.str()?;
        let n = r.len()?;
        let mut primitives = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let topology = Topology::from_code(r.u8()?)?;
            let vertex_count = r.u32()?;
            let na = r.len()?;
            let mut attributes = Vec::with_capacity(na.min(16));
            for _ in 0..na {
                let (c, k) = (r.u8()?, r.u8()?);
                let semantic = Semantic::from_code(c, k)?;
                let format = VertexFormat::from_code(r.u8()?)?;
                let data = r.bytes()?;
                if data.len() != vertex_count as usize * format.size() {
                    return Err(format!(
                        "{semantic:?} stream holds {} bytes, expected {} vertices of {}",
                        data.len(),
                        vertex_count,
                        format.size()
                    ));
                }
                attributes.push(VertexAttribute {
                    semantic,
                    format,
                    data,
                });
            }
            let indices = match r.u8()? {
                0 => None,
                _ => {
                    let b = r.bytes()?;
                    if b.len() % 4 != 0 {
                        return Err("index buffer is not whole u32s".into());
                    }
                    Some(b)
                }
            };
            let material = match r.u8()? {
                0 => None,
                _ => Some(AssetId(r.u128()?)),
            };
            let mut v = [0.0; 6];
            for x in &mut v {
                *x = r.f64()?;
            }
            primitives.push(Primitive {
                topology,
                vertex_count,
                attributes,
                indices,
                material,
                bounds: Aabb {
                    min: [v[0], v[1], v[2]],
                    max: [v[3], v[4], v[5]],
                },
            });
        }
        r.end()?;
        Ok(Self { name, primitives })
    }
}

/// A texture: RGBA8, a full mip chain, largest level first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureAsset {
    /// Width of level 0.
    pub width: u32,
    /// Height of level 0.
    pub height: u32,
    /// sRGB-encoded colour (`false`: linear data such as normals).
    pub srgb: bool,
    /// Level data, level 0 first; each `w * h * 4` bytes.
    pub levels: Vec<Bytes>,
}

impl AssetValue for TextureAsset {
    const KIND: &'static str = KIND_TEXTURE;

    fn encode(&self) -> Result<Bytes, String> {
        Ok(crate::ktx2w::write_rgba8(self))
    }

    fn decode(bytes: &Bytes) -> Result<Self, String> {
        crate::ktx2w::read_rgba8(bytes)
    }
}

/// How a material's alpha is used.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AlphaMode {
    /// Alpha ignored.
    Opaque,
    /// Alpha-tested against a cutoff.
    Mask(f64),
    /// Alpha-blended.
    Blend,
}

/// A metallic-roughness PBR material (glTF's core model).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialAsset {
    /// Display name.
    pub name: String,
    /// Linear base colour factor (RGBA).
    pub base_color: [f64; 4],
    /// Base colour texture.
    pub base_color_texture: Option<AssetId>,
    /// Metallic factor.
    pub metallic: f64,
    /// Roughness factor.
    pub roughness: f64,
    /// Metallic (B) / roughness (G) texture.
    pub metallic_roughness_texture: Option<AssetId>,
    /// Tangent-space normal map.
    pub normal_texture: Option<AssetId>,
    /// Normal map scale.
    pub normal_scale: f64,
    /// Occlusion texture (R).
    pub occlusion_texture: Option<AssetId>,
    /// Linear emissive factor.
    pub emissive: [f64; 3],
    /// Emissive texture.
    pub emissive_texture: Option<AssetId>,
    /// Alpha mode.
    pub alpha_mode: AlphaMode,
    /// Rendered from both sides.
    pub double_sided: bool,
}

impl Default for MaterialAsset {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0; 4],
            base_color_texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

fn ron_encode<T: Serialize>(v: &T) -> Result<Bytes, String> {
    let cfg = ron::ser::PrettyConfig::new().new_line("\n".to_string());
    ron::ser::to_string_pretty(v, cfg)
        .map(Bytes::from)
        .map_err(|e| e.to_string())
}

fn ron_decode<T: for<'de> Deserialize<'de>>(b: &Bytes) -> Result<T, String> {
    let text = std::str::from_utf8(b).map_err(|e| e.to_string())?;
    ron::from_str(text).map_err(|e| e.to_string())
}

impl AssetValue for MaterialAsset {
    const KIND: &'static str = KIND_MATERIAL;
    fn encode(&self) -> Result<Bytes, String> {
        ron_encode(self)
    }
    fn decode(bytes: &Bytes) -> Result<Self, String> {
        ron_decode(bytes)
    }
}

/// One node of a scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneNode {
    /// Display name.
    pub name: String,
    /// Its parent (index into [`SceneAsset::nodes`]).
    pub parent: Option<u32>,
    /// Local translation.
    pub translation: [f64; 3],
    /// Local rotation (unit quaternion, xyzw).
    pub rotation: [f64; 4],
    /// Local scale.
    pub scale: [f64; 3],
    /// The mesh it draws.
    pub mesh: Option<AssetId>,
}

/// A node hierarchy (a glTF scene; later a prefab).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneAsset {
    /// Display name.
    pub name: String,
    /// Nodes; a parent always precedes its children.
    pub nodes: Vec<SceneNode>,
}

impl SceneAsset {
    /// Each node's world transform as a column-major 4x4 `f64` matrix, in node order.
    #[must_use]
    pub fn world_matrices(&self) -> Vec<[f64; 16]> {
        let mut out: Vec<[f64; 16]> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let local = trs(n.translation, n.rotation, n.scale);
            let m = match n.parent.and_then(|p| out.get(p as usize)) {
                Some(parent) => mul(parent, &local),
                None => local,
            };
            out.push(m);
        }
        out
    }
}

/// Column-major TRS matrix.
#[must_use]
pub fn trs(t: [f64; 3], q: [f64; 4], s: [f64; 3]) -> [f64; 16] {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        (1.0 - 2.0 * (yy + zz)) * s[0],
        2.0 * (xy + wz) * s[0],
        2.0 * (xz - wy) * s[0],
        0.0,
        2.0 * (xy - wz) * s[1],
        (1.0 - 2.0 * (xx + zz)) * s[1],
        2.0 * (yz + wx) * s[1],
        0.0,
        2.0 * (xz + wy) * s[2],
        2.0 * (yz - wx) * s[2],
        (1.0 - 2.0 * (xx + yy)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// Column-major 4x4 product `a * b`.
#[must_use]
pub fn mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut o = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    o
}

/// Transform a point by a column-major matrix.
#[must_use]
pub fn transform(m: &[f64; 16], p: [f64; 3]) -> [f64; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

impl AssetValue for SceneAsset {
    const KIND: &'static str = KIND_SCENE;
    fn encode(&self) -> Result<Bytes, String> {
        ron_encode(self)
    }
    fn decode(bytes: &Bytes) -> Result<Self, String> {
        let s: Self = ron_decode(bytes)?;
        for (i, n) in s.nodes.iter().enumerate() {
            if n.parent.is_some_and(|p| p as usize >= i) {
                return Err(format!("node {i}'s parent does not precede it"));
            }
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32s(v: &[f32]) -> Bytes {
        Bytes::from(v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>())
    }

    #[test]
    fn mesh_round_trips_and_rejects_bad_streams() {
        let m = MeshAsset {
            name: "tri".into(),
            primitives: vec![Primitive {
                topology: Topology::Triangles,
                vertex_count: 3,
                attributes: vec![VertexAttribute {
                    semantic: Semantic::Position,
                    format: VertexFormat::Float32x3,
                    data: f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                }],
                indices: Some(Bytes::from(
                    [0u32, 1, 2]
                        .iter()
                        .flat_map(|i| i.to_le_bytes())
                        .collect::<Vec<_>>(),
                )),
                material: Some(AssetId(7)),
                bounds: Aabb {
                    min: [0.0; 3],
                    max: [1.0, 1.0, 0.0],
                },
            }],
        };
        let b = m.encode().expect("encodes");
        assert_eq!(MeshAsset::decode(&b), Ok(m.clone()));
        assert_eq!(m.primitives[0].positions()[1], [1.0, 0.0, 0.0]);
        assert_eq!(m.primitives[0].index_list(), vec![0, 1, 2]);
        let mut bad = m;
        bad.primitives[0].vertex_count = 4;
        assert!(MeshAsset::decode(&bad.encode().expect("encodes")).is_err());
    }

    #[test]
    fn scene_world_matrices_compose_parent_first() {
        let s = SceneAsset {
            name: "s".into(),
            nodes: vec![
                SceneNode {
                    name: "a".into(),
                    parent: None,
                    translation: [1.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [2.0; 3],
                    mesh: None,
                },
                SceneNode {
                    name: "b".into(),
                    parent: Some(0),
                    translation: [1.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                    mesh: None,
                },
            ],
        };
        let w = s.world_matrices();
        assert_eq!(transform(&w[1], [0.0; 3]), [3.0, 0.0, 0.0]);
        assert_eq!(
            SceneAsset::decode(&s.encode().expect("encodes")),
            Ok(s.clone())
        );
        let mut bad = s;
        bad.nodes[0].parent = Some(1);
        assert!(SceneAsset::decode(&bad.encode().expect("encodes")).is_err());
    }
}
