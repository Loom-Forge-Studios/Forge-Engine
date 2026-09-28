//! `test_wgsl_only` — WGSL is the only shading language, and every shader goes through
//! naga validation in `forge_gpu::shader` (Ch.9).
//!
//! Over every crate, plugin and tool in the workspace:
//! * no production source names `ShaderSource::` except `crates/forge-gpu/src/shader.rs`
//!   (so every module is created from naga-validated IR, with errors returned);
//! * no GLSL / HLSL / SPIR-V / MSL shader file exists;
//! * no manifest enables a non-WGSL front end (`wgpu` `glsl`/`spirv`, naga `glsl-in`/
//!   `spv-in`, `shaderc`).
//!
//! Positive control (W2): `positive_control_each_rule_flags_its_violation`.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const ALLOWED: &str = "crates/forge-gpu/src/shader.rs";
const FOREIGN_EXT: &[&str] = &[
    "glsl", "vert", "frag", "comp", "geom", "tesc", "tese", "hlsl", "spv", "metal",
];

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let p = e.path();
        if p.is_dir() {
            walk(&p, &r, out);
        } else {
            out.push((r, p));
        }
    }
}

/// Production Rust sources (under a `src/` directory) that name `ShaderSource::`.
fn shader_source_uses(files: &[(String, String)]) -> Vec<String> {
    files
        .iter()
        .filter(|(p, _)| p.ends_with(".rs") && p.contains("/src/") && p != ALLOWED)
        .flat_map(|(p, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, l)| {
                    l.split("//")
                        .next()
                        .unwrap_or("")
                        .contains("ShaderSource::")
                })
                .map(move |(i, l)| format!("{p}:{}: {}", i + 1, l.trim()))
        })
        .collect()
}

fn foreign_shader_files(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|p| {
            Path::new(p.as_str())
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| FOREIGN_EXT.contains(&e))
        })
        .cloned()
        .collect()
}

fn foreign_front_ends(manifests: &[(String, String)]) -> Vec<String> {
    let mut bad = Vec::new();
    for (p, text) in manifests {
        for (i, l) in text.lines().enumerate() {
            let code = l.split('#').next().unwrap_or("");
            let wgpu_or_naga =
                code.trim_start().starts_with("wgpu") || code.trim_start().starts_with("naga");
            let feature = [
                "\"glsl\"",
                "\"spirv\"",
                "\"glsl-in\"",
                "\"spv-in\"",
                "\"hlsl-in\"",
            ]
            .iter()
            .any(|f| code.contains(f));
            if (wgpu_or_naga && feature) || code.trim_start().starts_with("shaderc") {
                bad.push(format!("{p}:{}: {}", i + 1, l.trim()));
            }
        }
    }
    bad
}

/// (path, contents) pairs.
type Texts = Vec<(String, String)>;

fn scan() -> (Texts, Vec<String>, Texts) {
    let mut all = Vec::new();
    for top in ["crates", "plugins", "tools"] {
        walk(&root().join(top), top, &mut all);
    }
    let sources = all
        .iter()
        .filter(|(r, _)| r.ends_with(".rs"))
        .map(|(r, p)| (r.clone(), std::fs::read_to_string(p).unwrap_or_default()))
        .collect();
    let paths = all.iter().map(|(r, _)| r.clone()).collect();
    let manifests = all
        .iter()
        .filter(|(r, _)| r.ends_with("Cargo.toml"))
        .map(|(r, p)| (r.clone(), std::fs::read_to_string(p).unwrap_or_default()))
        .collect();
    (sources, paths, manifests)
}

#[test]
fn every_shader_is_wgsl_through_forge_gpu() {
    let (sources, paths, manifests) = scan();
    assert!(
        sources
            .iter()
            .any(|(p, t)| p == ALLOWED && t.contains("ShaderSource::Naga")),
        "the scan did not find forge-gpu's shader module — it would pass vacuously"
    );
    assert!(
        paths.iter().any(|p| p.ends_with(".wgsl")),
        "no .wgsl file found — the scan is looking in the wrong place"
    );
    assert!(
        manifests.len() > 10,
        "only {} manifests scanned",
        manifests.len()
    );
    let a = shader_source_uses(&sources);
    assert!(
        a.is_empty(),
        "shader modules created outside forge_gpu::shader:\n{}",
        a.join("\n")
    );
    let b = foreign_shader_files(&paths);
    assert!(b.is_empty(), "non-WGSL shader files:\n{}", b.join("\n"));
    let c = foreign_front_ends(&manifests);
    assert!(
        c.is_empty(),
        "non-WGSL shader front ends enabled:\n{}",
        c.join("\n")
    );
}

#[test]
fn positive_control_each_rule_flags_its_violation() {
    let src = vec![(
        "crates/forge-render/src/lib.rs".to_string(),
        "let m = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::SpirV(words) });".to_string(),
    )];
    assert_eq!(shader_source_uses(&src).len(), 1);
    assert!(
        shader_source_uses(&[(ALLOWED.to_string(), "ShaderSource::Naga".to_string())]).is_empty()
    );
    assert!(
        shader_source_uses(&[(
            "crates/x/tests/t.rs".to_string(),
            "ShaderSource::Wgsl".to_string()
        )])
        .is_empty(),
        "tests may create modules directly"
    );
    assert_eq!(
        foreign_shader_files(&["crates/forge-render/shaders/sky.frag".to_string()]).len(),
        1
    );
    assert_eq!(
        foreign_shader_files(&["plugins/x/a.spv".to_string(), "a.wgsl".to_string()]).len(),
        1
    );
    let m = vec![(
        "crates/forge-render/Cargo.toml".to_string(),
        "wgpu = { version = \"30\", features = [\"spirv\"] }\nshaderc = \"0.8\"\n".to_string(),
    )];
    assert_eq!(foreign_front_ends(&m).len(), 2);
}
