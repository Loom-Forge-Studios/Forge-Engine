//! `test_ui_wgpu_confined` (Ch.21 §21.23; D-3, D-1).
//!
//! * No `wgpu` path outside `crates/forge-ui/src/render_wgpu/`: nothing in `forge-ui`
//!   below its renderer module touches `wgpu`, so moving the renderer onto `forge-gpu`'s
//!   device pool changes one module.
//! * `egui` (or `eframe`) is absent from the resolved dependency graph (`Cargo.lock`).
//!
//! Positive controls: `use wgpu` added under `src/layout/`, and a lockfile containing
//! `egui`, both fail.

use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `(relative path, contents)` of Rust and WGSL sources under `src/`.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else if p.extension().is_some_and(|x| x == "rs" || x == "wgsl") {
                let rel = p
                    .strip_prefix(root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                out.push((rel, std::fs::read_to_string(&p).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    let root = crate_dir();
    walk(&root.join("src"), &root, &mut out);
    out
}

/// Lines outside `src/render_wgpu/` that use a `wgpu` path (comments excluded).
fn wgpu_uses(files: &[(String, String)]) -> Vec<String> {
    let mut bad = Vec::new();
    for (path, text) in files {
        if path.starts_with("src/render_wgpu/") {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if names_wgpu(code) {
                bad.push(format!("{path}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    bad
}

/// Whether a line of code names the `wgpu` crate: the identifier `wgpu` (not part of a
/// longer identifier such as `render_wgpu`) used as a path root or imported.
fn names_wgpu(code: &str) -> bool {
    let b = code.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    while let Some(off) = code[i..].find("wgpu") {
        let s = i + off;
        let e = s + 4;
        let before_ok = s == 0 || !ident(b[s - 1]);
        let after_ok = e >= b.len() || !ident(b[e]);
        if before_ok && after_ok {
            let rest = code[e..].trim_start();
            let head = code[..s].trim_end();
            if rest.starts_with("::") || head.ends_with("use") || head.ends_with("crate") {
                return true;
            }
        }
        i = e;
    }
    false
}

/// Packages in a lockfile whose name is an immediate-mode UI crate (D-1).
fn egui_packages(lock: &str) -> Vec<String> {
    lock.lines()
        .filter_map(|l| l.strip_prefix("name = \""))
        .map(|n| n.trim_end_matches('"').to_string())
        .filter(|n| {
            n == "egui" || n == "eframe" || n.starts_with("egui-") || n.starts_with("egui_")
        })
        .collect()
}

#[test]
fn wgpu_is_confined_to_render_wgpu() {
    let files = sources();
    assert!(
        files.iter().any(|(p, _)| p == "src/render_wgpu/mod.rs"),
        "the scan found no renderer module — it would pass vacuously"
    );
    assert!(
        files.len() > 20,
        "only {} source files scanned",
        files.len()
    );
    let bad = wgpu_uses(&files);
    assert!(
        bad.is_empty(),
        "wgpu named outside src/render_wgpu/:\n{}",
        bad.join("\n")
    );
}

#[test]
fn egui_is_absent_from_the_resolved_graph() {
    let lock_path = crate_dir().join("../../Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("{}: {e}", lock_path.display()));
    assert!(
        lock.contains("name = \"forge-ui\""),
        "not the workspace lockfile"
    );
    let found = egui_packages(&lock);
    assert!(
        found.is_empty(),
        "immediate-mode UI crates in Cargo.lock: {found:?}"
    );
}

#[test]
fn positive_control_wgpu_under_layout_fails() {
    let mut files = sources();
    files.push((
        "src/layout/sneaky.rs".into(),
        "use wgpu::Device;\nfn f(_d: &Device) {}\n".into(),
    ));
    let bad = wgpu_uses(&files);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].starts_with("src/layout/sneaky.rs:1"));
}

#[test]
fn positive_control_egui_in_lockfile_fails() {
    let lock =
        "[[package]]\nname = \"forge-ui\"\n\n[[package]]\nname = \"egui\"\nversion = \"0.30.0\"\n";
    assert_eq!(egui_packages(lock), vec!["egui".to_string()]);
}
