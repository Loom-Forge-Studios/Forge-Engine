//! Guard (Ch.1.1, Ch.3.1): `forge-num` depends on nothing — no crate, and not the platform's
//! transcendental functions.
//!
//! The crate-local `clippy.toml` already rejects `f64::sin` & co. at lint time; this test is
//! the second, independent layer, so the property survives someone deleting that file or
//! running without clippy. It scans every source file under `src/` for calls into the
//! platform libm (method or path form, f64 or f32) and FFI, and asserts the manifest's
//! `[dependencies]` table is empty.

use std::path::{Path, PathBuf};

/// Methods whose results the platform's libm decides.
const FORBIDDEN: &[&str] = &[
    "sin", "cos", "tan", "sin_cos", "asin", "acos", "atan", "atan2", "exp", "exp2", "exp_m1", "ln",
    "log", "log2", "log10", "ln_1p", "powf", "powi", "sinh", "cosh", "tanh", "asinh", "acosh",
    "atanh", "hypot", "cbrt", "mul_add",
];

/// Every violation in one source text, as `line: text`.
fn violations(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        // Ignore comments (doc text legitimately names `f64::sin`).
        let line = raw.split("//").next().unwrap_or("");
        let mut hit = |why: &str| out.push(format!("{}: {why}: {}", i + 1, raw.trim()));
        for name in FORBIDDEN {
            for pat in [
                format!(".{name}("),
                format!("f64::{name}("),
                format!("f32::{name}("),
                format!("f64::{name})"),
                format!("f32::{name})"),
                format!("f64::{name},"),
            ] {
                if line.contains(&pat) {
                    hit(&format!("platform libm `{name}`"));
                }
            }
        }
        for pat in ["extern \"C\"", "libm::", "#[link"] {
            if line.contains(pat) {
                hit("foreign math code");
            }
        }
    }
    out
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read src dir") {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The body of the manifest's `[dependencies]` table, minus comments and blank lines.
fn dependency_lines(manifest: &str) -> Vec<String> {
    let mut in_deps = false;
    let mut out = Vec::new();
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]" || t.starts_with("[dependencies.");
            continue;
        }
        if in_deps && !t.is_empty() && !t.starts_with('#') {
            out.push(t.to_string());
        }
    }
    out
}

#[test]
fn forge_num_calls_no_platform_libm() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rs_files(&root.join("src"), &mut files);
    assert!(
        files.len() >= 5,
        "found only {files:?}: the scan is looking in the wrong place"
    );
    let mut all = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        for v in violations(&src) {
            all.push(format!("{}:{v}", f.display()));
        }
    }
    assert!(
        all.is_empty(),
        "forge-num reaches the platform libm (Ch.3.1):\n{}",
        all.join("\n")
    );
}

#[test]
fn forge_num_has_no_dependencies() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("[dependencies]"),
        "the scan would be vacuous"
    );
    let deps = dependency_lines(&manifest);
    assert!(
        deps.is_empty(),
        "forge-num must depend on nothing (Ch.1.1): {deps:?}"
    );
}

// ---- positive controls (W2) ----

#[test]
fn positive_control_libm_calls_are_caught() {
    for bad in [
        "let y = x.sin();",
        "let y = f64::exp(x);",
        "let y = a.powf(b);",
        "let y = x.mul_add(a, b);",
        "xs.iter().map(f64::ln)",
        "let y = x.atan2(z);",
        "unsafe extern \"C\" { fn sin(x: f64) -> f64; }",
    ] {
        assert!(!violations(bad).is_empty(), "missed: {bad}");
    }
    // ...and clean code, including comments that name the functions, passes.
    assert!(violations("let y = det::sin(x); // not x.sin()\nlet s = x.sqrt();").is_empty());
}

#[test]
fn positive_control_a_dependency_is_caught() {
    let m = "[package]\nname = \"forge-num\"\n\n[dependencies]\n# none\nlibm = \"0.2\"\n\n[features]\nx = []\n";
    assert_eq!(dependency_lines(m), vec!["libm = \"0.2\"".to_string()]);
    let m = "[dependencies]\n# none\n\n[features]\nmutate-det = []\n";
    assert!(dependency_lines(m).is_empty());
}
