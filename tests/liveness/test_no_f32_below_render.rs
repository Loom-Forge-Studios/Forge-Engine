//! M1-3 / Ch.1.5 / Ch.2.4 — **no `f32` below `forge-render`, and inside it `f32` is made in
//! one place.**
//!
//! Three rules, each an AST check (syn) of the real sources:
//!
//! 1. **Below the renderer.** No crate that `forge-render` depends on (transitively, in the
//!    workspace — resolved from `cargo metadata`, not from a list that could drift), and no
//!    crate that declares itself below it (`[package.metadata.forge] below-render = true`: a
//!    simulation layer the renderer does not link), names `f32` in non-test code: no
//!    `f32` type, no `f32::` path, no `1.0f32` literal. The few decode sites that must read an
//!    `f32` from a file format and widen it at once are allow-listed per file in
//!    `tests/liveness/f32_allow.txt`, each with a reason; an entry that matches nothing fails.
//! 2. **The set is complete.** Every workspace crate in `forge-render`'s dependency closure is
//!    in the scanned set, so adding a dependency cannot open a hole.
//! 3. **The last mile.** Inside `forge-render/src`, every narrowing — an `as f32` cast or an
//!    `f32::from` call — is in `last_mile.rs`. There is no other path to the GPU.
//!
//! Strings are not code: WGSL's `vec4<f32>` inside a shader string is not flagged.
//!
//! Positive controls (W2): `positive_control_each_violation_is_flagged` feeds each rule a
//! source or manifest that breaks it (and a real engine file with a bad function appended)
//! and asserts it is flagged, and that test modules and strings are not.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use forge_tests::{DepGraph, workspace_root};
use syn::visit::Visit;

const ALLOW_FILE: &str = "tests/liveness/f32_allow.txt";

/// The core and the GPU layer: every crate the spine (Ch.1.1) puts below `forge-render`.
/// Those that do not exist yet are skipped; each is scanned the day it lands. A crate outside
/// this list joins the scan by declaring itself below the renderer ([`BELOW_METADATA`]).
const BELOW: &[&str] = &[
    "forge-num",
    // Under forge-gpu since WP-19 (its test-only `test-gpu-turn` feature takes the GPU turn).
    "forge-trace",
    "forge-seed",
    "forge-frames",
    "forge-core",
    "forge-reflect",
    "forge-reflect-macros",
    "forge-cmd",
    "forge-store",
    "forge-plugin",
    "forge-asset",
    "forge-gpu",
    "forge-sky",
    "forge-pcg",
    "forge-phys",
];

/// The `[package.metadata.forge]` key a crate sets to `true` to be scanned as below the
/// renderer (a simulation layer the renderer does not link, whose numbers must still be f64).
const BELOW_METADATA: &str = "below-render";

/// Every crate the scan covers: [`BELOW`] and every crate declaring [`BELOW_METADATA`].
fn below_crates(graph: &DepGraph) -> Vec<String> {
    let mut out: Vec<String> = BELOW.iter().map(|c| (*c).to_string()).collect();
    for c in graph.declaring(BELOW_METADATA) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Collects `f32` uses outside `#[cfg(test)]` items.
#[derive(Default)]
struct F32Uses {
    hits: Vec<String>,
    narrowings: Vec<String>,
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Meta>()
                .map(|m| quote::quote!(#m).to_string().contains("test"))
                .unwrap_or(false)
    })
}

impl<'ast> Visit<'ast> for F32Uses {
    fn visit_item(&mut self, i: &'ast syn::Item) {
        let attrs: &[syn::Attribute] = match i {
            syn::Item::Mod(m) => &m.attrs,
            syn::Item::Fn(f) => &f.attrs,
            syn::Item::Impl(x) => &x.attrs,
            syn::Item::Const(x) => &x.attrs,
            syn::Item::Struct(x) => &x.attrs,
            _ => &[],
        };
        if is_cfg_test(attrs) {
            return;
        }
        syn::visit::visit_item(self, i);
    }
    fn visit_path(&mut self, p: &'ast syn::Path) {
        if p.segments.first().is_some_and(|s| s.ident == "f32") {
            self.hits.push(quote::quote!(#p).to_string());
        }
        syn::visit::visit_path(self, p);
    }
    fn visit_lit_float(&mut self, l: &'ast syn::LitFloat) {
        if l.suffix() == "f32" {
            self.hits.push(l.to_string());
        }
    }
    fn visit_expr_cast(&mut self, c: &'ast syn::ExprCast) {
        if matches!(&*c.ty, syn::Type::Path(t) if t.path.is_ident("f32")) {
            self.narrowings.push(quote::quote!(#c).to_string());
        }
        syn::visit::visit_expr_cast(self, c);
    }
    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*c.func {
            let s: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|x| x.ident.to_string())
                .collect();
            if s == ["f32", "from"] {
                self.narrowings.push(quote::quote!(#c).to_string());
            }
        }
        syn::visit::visit_expr_call(self, c);
    }
}

fn scan(src: &str) -> F32Uses {
    let file = syn::parse_file(src).expect("parse");
    let mut v = F32Uses::default();
    v.visit_file(&file);
    v
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd {
        let p = e.expect("entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn allow_list(root: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(root.join(ALLOW_FILE)).expect("allow list");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (file, reason) = l
                .split_once('#')
                .expect("every allow entry needs a # reason");
            assert!(
                reason.trim().len() > 10,
                "allow entry {l:?} needs a real reason"
            );
            file.trim().to_string()
        })
        .collect()
}

/// The source directories of the crates below the renderer that exist, located through the
/// resolved graph (so a crate outside `crates/<name>`, like `forge-reflect-macros`, is found).
fn below_dirs(graph: &DepGraph) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for c in below_crates(graph) {
        for d in graph.dirs(&c).iter().filter(|d| d.local) {
            out.push(d.dir.join("src"));
        }
    }
    out
}

/// Rule 1: `f32` uses under `dirs`, as `(file, use)`.
fn below_violations(root: &Path, dirs: &[PathBuf]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        rs_files(dir, &mut files);
        for f in files {
            let src = std::fs::read_to_string(&f).expect("read");
            for h in scan(&src).hits {
                out.push((rel(root, &f), h));
            }
        }
    }
    out
}

/// Rule 3: narrowings in forge-render outside last_mile.rs.
fn narrowing_violations(files: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (name, src) in files {
        if name.ends_with("/last_mile.rs") {
            continue;
        }
        for n in scan(src).narrowings {
            out.push(format!("{name}: {n}"));
        }
    }
    out
}

/// Rule 2: workspace crates in forge-render's closure that are not scanned.
fn unscanned(graph: &DepGraph) -> Vec<String> {
    let closure = graph
        .closure("forge-render")
        .expect("forge-render in the graph");
    let scanned = below_crates(graph);
    closure
        .into_iter()
        .filter(|p| p != "forge-render")
        .filter(|p| graph.dirs(p).iter().any(|d| d.local))
        .filter(|p| !scanned.contains(p))
        .collect()
}

#[test]
fn no_f32_below_forge_render() {
    let root = workspace_root();
    let allow = allow_list(&root);
    let graph = DepGraph::resolve(&root).expect("cargo metadata");
    let dirs = below_dirs(&graph);
    // The crates the render layer stands on today are all found (the rest of BELOW lands later).
    for c in [
        "forge-num",
        "forge-frames",
        "forge-core",
        "forge-gpu",
        "forge-reflect-macros",
    ] {
        assert!(!graph.dirs(c).is_empty(), "{c} not in the workspace graph");
    }
    assert!(dirs.len() >= 10, "{dirs:?}");
    let hits = below_violations(&root, &dirs);
    let bad: Vec<_> = hits.iter().filter(|(f, _)| !allow.contains(f)).collect();
    assert!(bad.is_empty(), "f32 below forge-render (Ch.1.5): {bad:#?}");
    for a in &allow {
        assert!(
            hits.iter().any(|(f, _)| f == a),
            "{ALLOW_FILE}: {a} has no f32 left — remove the entry"
        );
    }
}

#[test]
fn every_workspace_crate_under_forge_render_is_scanned() {
    let graph = DepGraph::resolve(&workspace_root()).expect("cargo metadata");
    let missing = unscanned(&graph);
    assert!(
        missing.is_empty(),
        "forge-render depends on unscanned workspace crates: {missing:?}"
    );
}

#[test]
fn forge_render_narrows_only_in_last_mile() {
    let root = workspace_root();
    let mut files = Vec::new();
    rs_files(&root.join("crates/forge-render/src"), &mut files);
    assert!(files.iter().any(|f| f.ends_with("last_mile.rs")));
    let srcs: Vec<(String, String)> = files
        .iter()
        .map(|f| (rel(&root, f), std::fs::read_to_string(f).expect("read")))
        .collect();
    let bad = narrowing_violations(&srcs);
    assert!(
        bad.is_empty(),
        "narrowing to f32 outside last_mile.rs: {bad:#?}"
    );
    // And last_mile.rs really is where it happens.
    let lm = srcs
        .iter()
        .find(|(n, _)| n.ends_with("/last_mile.rs"))
        .expect("last_mile");
    assert!(!scan(&lm.1).narrowings.is_empty());
}

#[test]
fn positive_control_each_violation_is_flagged() {
    // Rule 1: a type, a path, a literal — flagged; a test module and a string — not.
    let src = r#"
        pub fn a(x: f32) -> f64 { f64::from(x) }
        pub fn b() -> f64 { f64::from(f32::EPSILON) }
        pub fn c() -> f64 { let y = 1.5f32; f64::from(y) }
        pub const WGSL: &str = "var<uniform> v: vec4<f32>;";
        #[cfg(test)]
        mod tests { fn t(z: f32) -> f32 { z } }
    "#;
    let s = scan(src);
    // `x: f32`, `f32::EPSILON`, `1.5f32` — and nothing from the string or the test module.
    assert_eq!(s.hits.len(), 3, "{:?}", s.hits);
    // A real engine file with one bad function appended is flagged.
    let root = workspace_root();
    let real = std::fs::read_to_string(root.join("crates/forge-num/src/linalg.rs")).expect("read");
    assert!(scan(&real).hits.is_empty());
    let bad = format!("{real}\npub fn narrow(x: f64) -> f32 {{ x as f32 }}\n");
    assert!(!scan(&bad).hits.is_empty());
    // Rule 3: a cast or f32::from outside last_mile.rs is flagged; inside it is not.
    let narrowing = "pub fn n(x: f64) -> f32 { x as f32 } pub fn m(x: u16) -> f32 { f32::from(x) }";
    let v = narrowing_violations(&[(
        "crates/forge-render/src/prepare.rs".into(),
        narrowing.into(),
    )]);
    assert_eq!(v.len(), 2, "{v:?}");
    let v = narrowing_violations(&[(
        "crates/forge-render/src/last_mile.rs".into(),
        narrowing.into(),
    )]);
    assert!(v.is_empty());
    // Rule 2: a workspace dependency outside the scanned set is reported.
    let meta = serde_json::json!({
        "packages": [
            {"id": "r", "name": "forge-render", "manifest_path": "/w/crates/forge-render/Cargo.toml", "source": null},
            {"id": "g", "name": "forge-gpu", "manifest_path": "/w/crates/forge-gpu/Cargo.toml", "source": null},
            {"id": "x", "name": "forge-sneaky", "manifest_path": "/w/crates/forge-sneaky/Cargo.toml", "source": null}
        ],
        "resolve": {"nodes": [
            {"id": "r", "deps": [{"pkg": "g", "dep_kinds": [{"kind": null}]}, {"pkg": "x", "dep_kinds": [{"kind": null}]}], "features": []},
            {"id": "g", "deps": [], "features": []},
            {"id": "x", "deps": [], "features": []}
        ]}
    });
    let g = DepGraph::from_metadata(&meta).expect("graph");
    assert_eq!(unscanned(&g), ["forge-sneaky"]);
    // A crate that declares itself below the renderer is scanned, linked by it or not; one
    // whose declaration is false is not.
    let meta = serde_json::json!({
        "packages": [
            {"id": "r", "name": "forge-render", "manifest_path": "/w/crates/forge-render/Cargo.toml", "source": null},
            {"id": "s", "name": "forge-sim-layer", "manifest_path": "/w/crates/forge-sim-layer/Cargo.toml", "source": null,
             "metadata": {"forge": {"below-render": true}}},
            {"id": "o", "name": "forge-other", "manifest_path": "/w/crates/forge-other/Cargo.toml", "source": null,
             "metadata": {"forge": {"below-render": false}}}
        ],
        "resolve": {"nodes": [
            {"id": "r", "deps": [], "features": []},
            {"id": "s", "deps": [], "features": []},
            {"id": "o", "deps": [], "features": []}
        ]}
    });
    let g = DepGraph::from_metadata(&meta).expect("graph");
    let scanned = below_crates(&g);
    assert!(
        scanned.iter().any(|c| c == "forge-sim-layer"),
        "{scanned:?}"
    );
    assert!(!scanned.iter().any(|c| c == "forge-other"), "{scanned:?}");
}

/// Rule 4 (WP-U15, Ch.35): the 2D pipeline keeps the same discipline inside one crate —
/// everything that simulates (physics, particles, skeletons, tiles, navigation, packing) is
/// `f64`; `f32` exists only under `forge-2d/src/render/`, and is made only in its
/// `last_mile.rs`. Returns the violations among `files` (`(path, source)`).
fn two_d_violations(files: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (name, src) in files {
        let s = scan(src);
        if !name.contains("/src/render/") {
            for h in s.hits {
                out.push(format!("{name}: f32 outside the 2D render path: {h}"));
            }
        }
    }
    out.extend(narrowing_violations(files));
    out
}

#[test]
fn forge_2d_keeps_f32_in_its_render_path() {
    let root = workspace_root();
    let mut files = Vec::new();
    rs_files(&root.join("crates/forge-2d/src"), &mut files);
    let srcs: Vec<(String, String)> = files
        .iter()
        .map(|f| (rel(&root, f), std::fs::read_to_string(f).expect("read")))
        .collect();
    assert!(srcs.iter().any(|(n, _)| n.ends_with("/src/physics/mod.rs")));
    let bad = two_d_violations(&srcs);
    assert!(bad.is_empty(), "{bad:#?}");
    let lm = srcs
        .iter()
        .find(|(n, _)| n.ends_with("/src/render/last_mile.rs"))
        .expect("the 2D last mile");
    assert!(
        !scan(&lm.1).narrowings.is_empty(),
        "the 2D last mile narrows"
    );
}

#[test]
fn positive_control_f32_in_2d_simulation_is_flagged() {
    let root = workspace_root();
    let real =
        std::fs::read_to_string(root.join("crates/forge-2d/src/physics/mod.rs")).expect("read");
    let clean = vec![(
        "crates/forge-2d/src/physics/mod.rs".to_string(),
        real.clone(),
    )];
    assert!(two_d_violations(&clean).is_empty());
    let bad = format!("{real}\npub fn fast_gravity(g: f32) -> f64 {{ f64::from(g) }}\n");
    let v = two_d_violations(&[("crates/forge-2d/src/physics/mod.rs".into(), bad)]);
    assert_eq!(v.len(), 1, "{v:?}");
    // A narrowing in the renderer but outside its last mile is flagged too.
    let v = two_d_violations(&[(
        "crates/forge-2d/src/render/mod.rs".into(),
        "pub fn n(x: f64) -> f32 { x as f32 }".into(),
    )]);
    assert_eq!(v.len(), 1, "{v:?}");
}
