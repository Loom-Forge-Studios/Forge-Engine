//! Synthetic glTF scenes, generated in memory: for the round-trip, hot-reload and import-perf
//! tests, and for tools that need a realistic scene without shipping binary fixtures in the
//! repository. Deterministic: the same parameters give the same bytes.

use bytes::Bytes;
use serde_json::json;

/// What to generate.
#[derive(Clone, Debug)]
pub struct SceneSpec {
    /// The `.gltf` file's name (its `.bin` is `<stem>.bin` beside it).
    pub name: String,
    /// Objects (nodes), each with its own mesh.
    pub objects: usize,
    /// Grid resolution of each mesh: `(grid+1)^2` vertices, `2*grid^2` triangles.
    pub grid: usize,
    /// Distinct materials, round-robin over objects.
    pub materials: usize,
    /// Give the materials a base-colour and a normal texture (external PNGs).
    pub textured: bool,
    /// Varies the geometry: a different seed moves every vertex.
    pub seed: u32,
}

impl Default for SceneSpec {
    fn default() -> Self {
        Self {
            name: "scene.gltf".into(),
            objects: 4,
            grid: 4,
            materials: 2,
            textured: true,
            seed: 0,
        }
    }
}

/// The generated files: `(path relative to the .gltf's directory, bytes)`, the `.gltf` first.
pub type Files = Vec<(String, Bytes)>;

/// A 2-colour checker PNG.
#[must_use]
pub fn checker_png(size: u32, a: [u8; 4], b: [u8; 4]) -> Bytes {
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            px.extend_from_slice(if ((x / 8) + (y / 8)) % 2 == 0 { &a } else { &b });
        }
    }
    let mut png = Vec::new();
    // Encoding into a Vec cannot fail for valid dimensions; an empty file would fail import.
    let _ = image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &px,
        size,
        size,
        image::ExtendedColorType::Rgba8,
    );
    Bytes::from(png)
}

/// Generate a scene.
#[must_use]
pub fn scene(spec: &SceneSpec) -> Files {
    let stem = spec
        .name
        .rsplit_once('.')
        .map_or(spec.name.as_str(), |(s, _)| s)
        .to_string();
    let bin_name = format!("{stem}.bin");
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    let g = spec.grid.max(1);
    let mut view = |bin: &mut Vec<u8>, data: &[u8], target: u32| {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": data.len(), "target": target }));
        bin.extend_from_slice(data);
        views.len() - 1
    };
    for o in 0..spec.objects {
        let mut pos = Vec::new();
        let mut nrm = Vec::new();
        let mut uv = Vec::new();
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        let wobble = ((o as u32).wrapping_mul(2_654_435_761) ^ spec.seed) % 97;
        for j in 0..=g {
            for i in 0..=g {
                let (u, v) = (i as f32 / g as f32, j as f32 / g as f32);
                let h = ((i * 7 + j * 13 + wobble as usize) % 11) as f32 * 0.02;
                let p = [u - 0.5, h, v - 0.5];
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
                pos.extend(p.iter().flat_map(|x| x.to_le_bytes()));
                nrm.extend([0.0f32, 1.0, 0.0].iter().flat_map(|x| x.to_le_bytes()));
                uv.extend([u, v].iter().flat_map(|x| x.to_le_bytes()));
            }
        }
        let mut idx = Vec::new();
        let w = (g + 1) as u32;
        for j in 0..g as u32 {
            for i in 0..g as u32 {
                let a = j * w + i;
                for t in [a, a + w, a + 1, a + 1, a + w, a + w + 1] {
                    idx.extend(t.to_le_bytes());
                }
            }
        }
        let n = (g + 1) * (g + 1);
        let pv = view(&mut bin, &pos, 34962);
        let nv = view(&mut bin, &nrm, 34962);
        let tv = view(&mut bin, &uv, 34962);
        let iv = view(&mut bin, &idx, 34963);
        let base = accessors.len();
        accessors.push(
            json!({ "bufferView": pv, "componentType": 5126, "count": n, "type": "VEC3",
            "min": lo.map(f64::from), "max": hi.map(f64::from) }),
        );
        accessors
            .push(json!({ "bufferView": nv, "componentType": 5126, "count": n, "type": "VEC3" }));
        accessors
            .push(json!({ "bufferView": tv, "componentType": 5126, "count": n, "type": "VEC2" }));
        accessors.push(json!({ "bufferView": iv, "componentType": 5125, "count": idx.len() / 4, "type": "SCALAR" }));
        let mut prim = json!({
            "attributes": { "POSITION": base, "NORMAL": base + 1, "TEXCOORD_0": base + 2 },
            "indices": base + 3,
        });
        if spec.materials > 0 {
            prim["material"] = json!(o % spec.materials);
        }
        meshes.push(json!({ "name": format!("Mesh_{o}"), "primitives": [prim] }));
        let (x, z) = ((o % 25) as f64 * 1.5, (o / 25) as f64 * 1.5);
        nodes.push(json!({
            "name": format!("Object_{o}"),
            "mesh": o,
            "translation": [x, 0.0, z],
        }));
    }
    let mut files: Files = Vec::new();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "forge-asset fixture" },
        "scene": 0,
        "scenes": [{ "name": stem, "nodes": (0..spec.objects).collect::<Vec<_>>() }],
        "nodes": nodes,
        "meshes": meshes,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{ "uri": bin_name, "byteLength": bin.len() }],
    });
    if spec.materials > 0 {
        let mats: Vec<_> = (0..spec.materials)
            .map(|m| {
                let mut j = json!({
                    "name": format!("Material_{m}"),
                    "pbrMetallicRoughness": {
                        "baseColorFactor": [1.0, 0.5 + 0.5 * (m % 2) as f64, 0.25, 1.0],
                        "metallicFactor": 0.0,
                        "roughnessFactor": 0.5,
                    },
                });
                if spec.textured {
                    j["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": 0 });
                    j["normalTexture"] = json!({ "index": 1 });
                }
                j
            })
            .collect();
        doc["materials"] = json!(mats);
    }
    if spec.textured {
        doc["images"] = json!([
            { "name": "Checker", "uri": "textures/checker.png" },
            { "name": "Normal", "uri": "textures/normal.png" },
        ]);
        doc["textures"] = json!([{ "source": 0 }, { "source": 1 }]);
    }
    files.push((
        spec.name.clone(),
        Bytes::from(serde_json::to_vec_pretty(&doc).unwrap_or_default()),
    ));
    files.push((bin_name, Bytes::from(bin)));
    if spec.textured {
        files.push((
            "textures/checker.png".into(),
            checker_png(64, [230, 60, 40, 255], [250, 240, 230, 255]),
        ));
        files.push((
            "textures/normal.png".into(),
            checker_png(32, [128, 128, 255, 255], [140, 120, 250, 255]),
        ));
    }
    files
}
