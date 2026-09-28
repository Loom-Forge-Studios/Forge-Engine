//! `test_game_ui_links_no_editor` (Ch.21 §21.20, §21.23; DoD M2-29): **a shipped game's UI
//! is `forge-ui` alone.** `forge-runtime` links `forge-ui` (with its `wgpu` renderer and
//! `winit` runner) and **no editor crate**: not `forge-editor`, not any `forge-panels-*`,
//! not `forge-licence` (I21's guard excludes that one too).
//!
//! The guard walks `forge-runtime`'s transitive dependency graph **as cargo resolves it**
//! (`cargo metadata`, host platform, the features a build of `forge-runtime` alone enables)
//! and fails on an editor crate anywhere in it — by name, by the `forge-panels-` prefix, and
//! by *location*: any local crate whose manifest lives under `plugins/forge-panels-*` or
//! `crates/forge-editor`, so a renamed panel crate is still caught (W3). Each finding names
//! the dependency path that pulls it in.
//!
//! Non-vacuity: the runtime really does link `forge-ui`, with the `wgpu` and `winit`
//! features on (a runtime that dropped its game UI would pass trivially otherwise).
//!
//! Positive controls (W2): synthetic graphs (a direct editor dependency, one reached through
//! `forge-ui`, a panel crate, a renamed crate under `plugins/forge-panels-*`) must fail;
//! and a scratch workspace that is the real `forge-runtime` manifest plus a dependency on
//! the real `crates/forge-editor`, resolved by real cargo, must fail — while the same
//! scratch workspace without it passes.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use forge_tests::{DepGraph, workspace_root};

const RUNTIME: &str = "forge-runtime";

/// Editor crates by name (§21.20).
const EDITOR_CRATES: &[&str] = &["forge-editor", "forge-licence", "forge-license"];
/// Editor crates by prefix.
const EDITOR_PREFIXES: &[&str] = &["forge-panels-"];
/// Editor crates by where they live (relative to the workspace root, `/`-separated).
const EDITOR_DIRS: &[&str] = &["crates/forge-editor", "plugins/forge-panels-"];

/// The shortest dependency path from `root` to `target` (`a -> b -> c`).
fn path_to(g: &DepGraph, root: &str, target: &str) -> String {
    let mut prev: BTreeMap<String, String> = BTreeMap::new();
    let mut q = VecDeque::from([root.to_string()]);
    let mut seen = BTreeSet::from([root.to_string()]);
    while let Some(p) = q.pop_front() {
        if p == target {
            let mut chain = vec![p.clone()];
            let mut cur = p;
            while let Some(b) = prev.get(&cur) {
                chain.push(b.clone());
                cur = b.clone();
            }
            chain.reverse();
            return chain.join(" -> ");
        }
        for d in g.deps(&p) {
            if seen.insert(d.clone()) {
                prev.insert(d.clone(), p.clone());
                q.push_back(d);
            }
        }
    }
    target.to_string()
}

fn rel(dir: &Path, root: &Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or(dir)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Every editor crate in `forge-runtime`'s closure, with the path that pulls it in.
fn violations(g: &DepGraph, root_dir: &Path) -> Result<Vec<String>, String> {
    let closure = g.closure(RUNTIME)?;
    let mut out = Vec::new();
    for pkg in &closure {
        let by_name = EDITOR_CRATES.contains(&pkg.as_str())
            || EDITOR_PREFIXES.iter().any(|p| pkg.starts_with(p));
        let by_dir = g.dirs(pkg).iter().find(|d| {
            let r = rel(&d.dir, root_dir);
            d.local && EDITOR_DIRS.iter().any(|e| r.starts_with(e))
        });
        if by_name || by_dir.is_some() {
            let why = match by_dir {
                Some(d) if !by_name => format!(" (it lives in {})", rel(&d.dir, root_dir)),
                _ => String::new(),
            };
            out.push(format!(
                "{pkg}: an editor crate in the shipped runtime{why}: {}",
                path_to(g, RUNTIME, pkg)
            ));
        }
    }
    Ok(out)
}

/// The runtime's graph with the features a build of `forge-runtime` alone enables.
fn runtime_graph(root: &Path, extra: &[&str]) -> DepGraph {
    DepGraph::resolve_with(root, extra)
        .and_then(|g| g.with_features_of(root, RUNTIME, extra))
        .expect("resolve forge-runtime")
}

#[test]
fn forge_runtime_links_forge_ui_and_no_editor_crate() {
    let root = workspace_root();
    let g = runtime_graph(&root, &["--locked"]);
    let v = violations(&g, &root).expect("forge-runtime is in the graph");
    assert!(v.is_empty(), "§21.20 violated:\n{}", v.join("\n"));
    // Non-vacuity: the game UI is really there, renderer and runner included.
    let c = g.closure(RUNTIME).expect("closure");
    for must in [
        "forge-ui",
        "taffy",
        "cosmic-text",
        "accesskit",
        "winit",
        "wgpu",
    ] {
        assert!(
            c.contains(must),
            "forge-runtime does not link {must}: {c:?}"
        );
    }
    let f = g.features("forge-ui");
    for feat in ["wgpu", "winit"] {
        assert!(
            f.contains(feat),
            "forge-ui without `{feat}` in the runtime: {f:?}"
        );
    }
    assert!(c.len() > 40, "suspiciously small closure: {}", c.len());
}

// ---- positive controls ----------------------------------------------------------------------

/// Synthetic `cargo metadata`: `(name, dir relative to a fake root)`, edges.
fn meta(root: &Path, pkgs: &[(&str, &str)], edges: &[(&str, &str)]) -> serde_json::Value {
    let packages: Vec<_> = pkgs
        .iter()
        .map(|(n, dir)| {
            serde_json::json!({
                "id": n, "name": n,
                "manifest_path": root.join(dir).join("Cargo.toml").display().to_string(),
                "source": serde_json::Value::Null,
            })
        })
        .collect();
    let nodes: Vec<_> = pkgs
        .iter()
        .map(|(n, _)| {
            let deps: Vec<_> = edges
                .iter()
                .filter(|(a, _)| a == n)
                .map(|(_, b)| serde_json::json!({"pkg": b, "dep_kinds": [{"kind": null}]}))
                .collect();
            serde_json::json!({"id": n, "deps": deps, "features": []})
        })
        .collect();
    serde_json::json!({"packages": packages, "resolve": {"nodes": nodes}})
}

fn run(pkgs: &[(&str, &str)], edges: &[(&str, &str)]) -> Vec<String> {
    let root = PathBuf::from("/fake/ws");
    let g = DepGraph::from_metadata(&meta(&root, pkgs, edges)).expect("synthetic metadata");
    violations(&g, &root).expect("runtime present")
}

#[test]
fn positive_control_synthetic_runtime_graphs_with_editor_crates_fail() {
    let base = [
        (RUNTIME, "crates/forge-runtime"),
        ("forge-ui", "crates/forge-ui"),
        ("forge-core", "crates/forge-core"),
    ];
    let base_edges = [(RUNTIME, "forge-ui"), (RUNTIME, "forge-core")];
    // Control of the control: the clean shape passes.
    assert!(run(&base, &base_edges).is_empty());

    // A direct dependency on the editor.
    let mut p = base.to_vec();
    p.push(("forge-editor", "crates/forge-editor"));
    let mut e = base_edges.to_vec();
    e.push((RUNTIME, "forge-editor"));
    let v = run(&p, &e);
    assert!(
        v.iter()
            .any(|l| l.starts_with("forge-editor:") && l.ends_with("forge-runtime -> forge-editor")),
        "{v:?}"
    );

    // Reached through forge-ui (a UI crate that grew an editor dependency).
    let mut e = base_edges.to_vec();
    e.push(("forge-ui", "forge-editor"));
    let v = run(&p, &e);
    assert!(
        v.iter()
            .any(|l| l.ends_with("forge-runtime -> forge-ui -> forge-editor")),
        "{v:?}"
    );

    // A panel crate, and forge-licence.
    let mut p = base.to_vec();
    p.push(("forge-panels-authoring", "plugins/forge-panels-authoring"));
    p.push(("forge-licence", "crates/forge-licence"));
    let mut e = base_edges.to_vec();
    e.push((RUNTIME, "forge-panels-authoring"));
    e.push(("forge-core", "forge-licence"));
    let v = run(&p, &e);
    assert!(
        v.iter().any(|l| l.starts_with("forge-panels-authoring:")),
        "{v:?}"
    );
    assert!(v.iter().any(|l| l.starts_with("forge-licence:")), "{v:?}");

    // A panel crate under another name is caught by where it lives.
    let mut p = base.to_vec();
    p.push(("sneaky-widgets", "plugins/forge-panels-sneaky"));
    let mut e = base_edges.to_vec();
    e.push(("forge-ui", "sneaky-widgets"));
    let v = run(&p, &e);
    assert!(
        v.iter()
            .any(|l| l.starts_with("sneaky-widgets:") && l.contains("plugins/forge-panels-sneaky")),
        "{v:?}"
    );
}

/// A scratch workspace that is the real `forge-runtime` manifest (its path dependencies
/// pointing at the real crates), optionally plus a dependency on the real `forge-editor`,
/// resolved by real cargo.
fn mutant(name: &str, with_editor: bool) -> (DepGraph, PathBuf) {
    let root = workspace_root();
    let ws = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("game-ui-no-editor")
        .join(name);
    let _ = std::fs::remove_dir_all(&ws);
    let rt = ws.join("forge-runtime");
    std::fs::create_dir_all(rt.join("src")).expect("mkdir");
    let crates = root.join("crates").display().to_string().replace('\\', "/");
    let real = std::fs::read_to_string(root.join("crates/forge-runtime/Cargo.toml"))
        .expect("forge-runtime manifest");
    let mut manifest = real
        .replace("\r\n", "\n")
        .replace("path = \"../", &format!("path = \"{crates}/"));
    // Dev-dependencies are not part of a shipped build (and would need their own crates).
    if let Some(i) = manifest.find("[dev-dependencies]") {
        let end = manifest[i + 1..]
            .find("\n[")
            .map_or(manifest.len(), |j| i + 1 + j);
        manifest.replace_range(i..end, "");
    }
    if with_editor {
        manifest = manifest.replace(
            "[dependencies]\n",
            &format!("[dependencies]\nforge-editor = {{ path = \"{crates}/forge-editor\" }}\n"),
        );
    }
    let root_toml = std::fs::read_to_string(root.join("Cargo.toml")).expect("root manifest");
    let root_toml = root_toml
        .lines()
        .map(|l| {
            if l.starts_with("members") {
                "members = [\"forge-runtime\"]".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(ws.join("Cargo.toml"), root_toml).expect("write");
    std::fs::write(rt.join("Cargo.toml"), manifest).expect("write");
    std::fs::write(rt.join("src/lib.rs"), "").expect("write");
    std::fs::copy(root.join("Cargo.lock"), ws.join("Cargo.lock")).expect("lock");
    (runtime_graph(&ws, &["--offline"]), root)
}

#[test]
fn positive_control_a_runtime_depending_on_forge_editor_fails() {
    // Control of the control: the scratch copy of the real runtime passes.
    let (base, root) = mutant("base", false);
    let v = violations(&base, &root).expect("runtime");
    assert!(v.is_empty(), "the scratch runtime should be clean: {v:?}");
    assert!(base.closure(RUNTIME).expect("closure").contains("forge-ui"));

    let (bad, root) = mutant("editor", true);
    let v = violations(&bad, &root).expect("runtime");
    assert!(
        v.iter()
            .any(|l| l.starts_with("forge-editor:") && l.contains("forge-runtime -> forge-editor")),
        "a runtime linking forge-editor must fail: {v:?}"
    );
}
