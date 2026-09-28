//! `cargo xtask gpu-turn` — WP-19: every crate with GPU tests must declare
//! `test-gpu-turn` on `forge-gpu` in `[dev-dependencies]` (or enable it via the
//! feature chain) so `cargo test -p <crate>` also takes the machine-wide GPU turn
//! (ADR 0041/0042).
//!
//! A crate "depends on forge-gpu" for the guard when forge-gpu appears in
//! `[dependencies]` (always compiled) or `[dev-dependencies]`. The guard requires that
//! the test-gpu-turn feature is on in every such entry: if only [dev-deps] has it, the
//! feature activates only during test builds; if only [dependencies] has it (non-optional),
//! that is always compiled (including tests) and must also carry test-gpu-turn.

use std::path::{Path, PathBuf};

use crate::util::{Failure, XResult};

/// All dependencies and dev-dependencies that mention forge-gpu.
struct GpuEntry {
    /// Table label.
    table_label: String,
    /// Whether forge-gpu is always compiled (non-optional) in this entry.
    always_in_test_build: bool,
    /// The features list (may be empty).
    features: Vec<String>,
}

/// Collect all forge-gpu entries from a manifest.
fn collect_gpu_entries(src: &str, manifest_rel: &str) -> Vec<GpuEntry> {
    let table: toml::Table = match src.parse() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut entries = Vec::new();

    entries.extend(check_dep_for_gpu(&table, "dependencies", manifest_rel));
    entries.extend(check_dep_for_gpu(&table, "dev-dependencies", manifest_rel));
    entries.extend(check_dep_for_gpu(
        &table,
        "build-dependencies",
        manifest_rel,
    ));

    if let Some(toml::Value::Table(target)) = table.get("target") {
        for (cfg, v) in target {
            let prefix = format!("target.{}", cfg);
            entries.extend(check_target_deps(v, &prefix, manifest_rel));
        }
    }

    entries
}

fn check_dep_for_gpu(table: &toml::Table, key: &str, _manifest_rel: &str) -> Vec<GpuEntry> {
    let mut entries = Vec::new();
    let deps = match table.get(key) {
        Some(toml::Value::Table(deps)) => deps,
        _ => return entries,
    };
    for (name, spec) in deps {
        let toml::Value::Table(spec) = spec else {
            continue;
        };
        let crate_name = spec
            .get("package")
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .to_string();
        if crate_name != "forge-gpu" {
            continue;
        }
        let always_in_build = spec.get("optional").and_then(|v| v.as_bool()) != Some(true);
        let features = spec
            .get("features")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        entries.push(GpuEntry {
            table_label: format!("[{}]", key),
            always_in_test_build: always_in_build,
            features,
        });
    }
    entries
}

fn check_target_deps(v: &toml::Value, prefix: &str, _manifest_rel: &str) -> Vec<GpuEntry> {
    let toml::Value::Table(tbl) = v else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let keys = ["dependencies", "build-dependencies", "dev-dependencies"];
    for key in &keys {
        let deps_table = tbl.get(*key);
        let deps = match deps_table {
            Some(toml::Value::Table(t)) => t,
            _ => continue,
        };
        let label = format!("[target.{}.{}]", prefix, key);
        for (name, spec) in deps {
            // match on a reference from `deps` (which is `&Map`)
            let toml::Value::Table(spec) = spec else {
                continue;
            };
            let crate_name = spec
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or(name)
                .to_string();
            if crate_name != "forge-gpu" {
                continue;
            }
            let always_in_build = spec.get("optional").and_then(|v| v.as_bool()) != Some(true);
            let features = spec
                .get("features")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            entries.push(GpuEntry {
                table_label: label.clone(),
                always_in_test_build: always_in_build,
                features,
            });
        }
    }
    entries
}

/// Check that all forge-gpu entries are correct (have test-gpu-turn).
fn check_entries(entries: &[GpuEntry], manifest_rel: &str) -> Vec<String> {
    let mut errs = Vec::new();

    // Cargo merges features from [dependencies] and [dev-dependencies].
    // If forge-gpu is in BOTH with test-gpu-turn on dev-dep side, that covers the always-on entry.
    let dev_deps_has_turn = entries
        .iter()
        .filter(|e| e.table_label.contains("dev-dep"))
        .any(|e| e.features.iter().any(|f| f == "test-gpu-turn"));

    let has_both_entries = entries
        .iter()
        .any(|e| e.table_label.contains("[dependencies]"))
        && entries.iter().any(|e| e.table_label.contains("dev-dep"));

    // Dev-deps entries: must have test-gpu-turn directly (no merging from dependencies).
    for e in entries.iter() {
        if e.table_label.contains("dev-dep") && !e.features.iter().any(|f| f == "test-gpu-turn") {
            errs.push(format!(
                "error: scratch/forge-gpu in {}: add features = [\"test-gpu-turn\"] (WP-19)",
                e.table_label
            ));
        }
    }

    // Always-on [dependencies] entries: need test-gpu-turn, unless dev-dep covers them.
    for e in entries.iter() {
        if e.table_label.contains("dev-dep") || !e.always_in_test_build {
            continue;
        }
        if has_both_entries && dev_deps_has_turn {
            // Dev-deps covers this one via Cargo feature merging.
            continue;
        }
        errs.push(format!(
            "error: scratch/forge-gpu in {} (of {}) needs test-gpu-turn (WP-19)",
            e.table_label, manifest_rel
        ));
    }

    errs
}

fn member_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    let dirs_list = ["crates", "plugins", "tools", "samples"];
    for d in &dirs_list {
        let p = root.join(d);
        if p.is_dir() {
            dirs.push((d.to_string(), p));
        }
    }
    dirs
}

/// Workspace members that are a single crate, not a directory of crates (`xtask`, `tests`,
/// `tests-premium` in this workspace's `[workspace].members`, D-6): their manifest sits directly at
/// `<root>/<name>/Cargo.toml`, so they need their own (non-glob) lookup rather than the
/// `crates/*`-style directory scan `member_dirs` does.
const SINGLE_CRATE_MEMBERS: &[&str] = &["xtask", "tests", "tests-premium"];

/// Check one manifest's forge-gpu entries and push any errors, prefixed with its path.
fn check_one_manifest(member_manifest: &Path, root: &Path, errs: &mut Vec<String>) {
    let Ok(member_src) = std::fs::read_to_string(member_manifest) else {
        return;
    };
    let member_manifest_rel = member_manifest
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| member_manifest.to_string_lossy().replace('\\', "/"));

    if member_src.parse::<toml::Table>().is_err() {
        return;
    }

    let entries = collect_gpu_entries(&member_src, &member_manifest_rel);

    let relevant: Vec<GpuEntry> = entries
        .into_iter()
        .filter(|e| {
            e.table_label.contains("dev-dep")
                || (e.always_in_test_build
                    && (e.table_label.contains("dependencies")
                        || e.table_label.contains("build-depend")))
        })
        .collect();

    if relevant.is_empty() {
        return;
    }

    let member_errs = check_entries(&relevant, &member_manifest_rel);
    for e in member_errs {
        errs.push(format!("{}: {}", member_manifest_rel, e));
    }
}

pub fn check_all(root: &Path) -> Vec<String> {
    let mut errs = Vec::new();
    let manifest = root.join("Cargo.toml");
    let src = match crate::util::read(&manifest) {
        Ok(s) => s,
        Err(_) => return errs,
    };
    let root_table: toml::Table = match src.parse() {
        Ok(t) => t,
        Err(_) => return errs,
    };

    let member_globs: Vec<&str> = match root_table.get("workspace").and_then(|w| w.get("members")) {
        Some(toml::Value::Array(globs)) => globs.iter().filter_map(|v| v.as_str()).collect(),
        _ => return errs,
    };

    let dirs = member_dirs(root);

    for (base_dir, base_path) in &dirs {
        let Ok(rd) = std::fs::read_dir(base_path) else {
            continue;
        };
        for entry in rd.filter_map(Result::ok) {
            let member_dir = entry.path();
            let member_manifest = member_dir.join("Cargo.toml");
            if !member_manifest.is_file() {
                continue;
            }

            let dir_name = member_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let full_name = format!("{}/{}", base_dir, dir_name);
            if !member_globs
                .iter()
                .any(|glob| matches_glob(glob, &full_name))
            {
                continue;
            }

            check_one_manifest(&member_manifest, root, &mut errs);
        }
    }

    // Single-crate members (`xtask`, `tests`): matched by exact name in `[workspace].members`,
    // not a glob, and their manifest is not nested a level down like the directories above.
    for name in SINGLE_CRATE_MEMBERS {
        if !member_globs.iter().any(|g| g == name) {
            continue;
        }
        let member_manifest = root.join(name).join("Cargo.toml");
        if member_manifest.is_file() {
            check_one_manifest(&member_manifest, root, &mut errs);
        }
    }

    errs
}

/// `cargo xtask gpu-turn`.
pub fn run(root: &Path) -> XResult<String> {
    let errs = check_all(root);
    if errs.is_empty() {
        Ok("gpu-turn: all GPU-test crates declare `test-gpu-turn` on forge-gpu\n".to_string())
    } else {
        let err_text: String = errs.join("\n");
        Err(Failure::one(err_text))
    }
}

fn matches_glob(glob: &str, full_name: &str) -> bool {
    match glob {
        "crates/*" => full_name.starts_with("crates/") && full_name.len() > "crates/".len(),
        "plugins/*" => full_name.starts_with("plugins/") && full_name.len() > "plugins/".len(),
        "tools/*" => full_name.starts_with("tools/") && full_name.len() > "tools/".len(),
        "samples/*" => full_name.starts_with("samples/") && full_name.len() > "samples/".len(),
        _ => false,
    }
}

/// Check one manifest string directly (used by the positive control test).
pub fn check_manifest(src: &str) -> Vec<String> {
    let entries = collect_gpu_entries(src, "scratch/Cargo.toml");
    let relevant: Vec<GpuEntry> = entries
        .into_iter()
        .filter(|e| {
            e.table_label.contains("dev-dep")
                || (e.always_in_test_build
                    && (e.table_label.contains("dependencies")
                        || e.table_label.contains("build-depend")))
        })
        .collect();
    check_entries(&relevant, "scratch/Cargo.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    #[test]
    fn a_valid_manifest_with_test_gpu_turn_passes() {
        let good = "[package]\nname = \"x\"\n[dev-dependencies]\nforge-gpu = { path = \".\", features = [\"test-gpu-turn\"] }\n";
        assert!(check_manifest(good).is_empty());
    }

    #[test]
    fn a_missing_test_gpu_turn_in_dev_deps_is_caught() {
        let bad = "[package]\nname = \"x\"\n[dev-dependencies]\nforge-gpu = { path = \".\" }\n";
        let errs = check_manifest(bad);
        assert!(!errs.is_empty(), "missing test-gpu-turn must be caught");
    }

    #[test]
    fn forge_gpu_with_wrong_features_is_caught() {
        let bad = "[package]\nname = \"x\"\n[dev-dependencies]\nforge-gpu = { path = \".\", features = [\"other\"] }\n";
        let errs = check_manifest(bad);
        assert!(!errs.is_empty(), "wrong features must be caught");
    }

    #[test]
    fn target_specific_dev_deps_are_checked() {
        let bad = "[package]\nname = \"x\"\n[target.'cfg(windows)'.dev-dependencies]\nforge-gpu = { path = \".\" }\n";
        let errs = check_manifest(bad);
        assert!(
            !errs.is_empty(),
            "forge-gpu in target-specific dev-deps must be caught"
        );
    }

    #[test]
    fn the_committed_repository_holds_the_guard() {
        let errs = check_all(&workspace_root());
        assert!(
            errs.is_empty(),
            "All crates with forge-gpu in deps must have test-gpu-turn:\n{}",
            errs.join("\n")
        );
    }
}
