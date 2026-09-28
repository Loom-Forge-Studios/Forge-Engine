//! Shared harness for the repo-level guard suites under `tests/<suite>/` (Ch.30).
//!
//! The guards that must reason about the *workspace* rather than one crate — I21's
//! `test_no_runtime_phone_home` (M0-19), the asset-reference case lint (M0-17), the liveness
//! family — need two things no single crate provides: the workspace root, and the resolved
//! dependency graph as cargo actually resolves it (features and all). Both are here, tested,
//! so those guards are built on a verified base rather than on a name match (W3).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The workspace root (the directory containing the root `Cargo.toml`).
pub fn workspace_root() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| here.to_path_buf())
}

/// The resolved dependency graph of the workspace, for the host platform, with default
/// features — what a real build links.
#[derive(Debug, Clone, Default)]
pub struct DepGraph {
    /// package name → names of its direct normal (non-dev, non-build) dependencies.
    edges: BTreeMap<String, BTreeSet<String>>,
    /// package name → its enabled features, as cargo resolved them.
    features: BTreeMap<String, BTreeSet<String>>,
    /// package name → where each resolved version of it lives on disk.
    dirs: BTreeMap<String, Vec<PkgDir>>,
    /// package name → the `true` keys of its `[package.metadata.forge]` table (what a crate
    /// declares about itself for the guards: `below-render`, ...).
    declared: BTreeMap<String, BTreeSet<String>>,
}

/// Where one resolved package lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgDir {
    /// The directory holding its `Cargo.toml`.
    pub dir: PathBuf,
    /// Built from a local path (a workspace member or a path dependency: `source` is null
    /// in the metadata), as opposed to a registry or git checkout.
    pub local: bool,
}

impl DepGraph {
    /// Resolve via `cargo metadata` at `root`.
    pub fn resolve(root: &Path) -> Result<Self, String> {
        Self::resolve_with(root, &["--locked"])
    }

    /// Resolve via `cargo metadata` at `root` with extra cargo flags (`--offline` for a
    /// scratch workspace a positive control builds).
    pub fn resolve_with(root: &Path, extra: &[&str]) -> Result<Self, String> {
        let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["metadata", "--format-version", "1"])
            .args(extra)
            .arg("--filter-platform")
            .arg(host_triple()?)
            .current_dir(root)
            .output()
            .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("cargo metadata emitted invalid JSON: {e}"))?;
        Self::from_metadata(&v)
    }

    /// Build from `cargo metadata` JSON. Split out so tests can feed synthetic graphs.
    pub fn from_metadata(v: &serde_json::Value) -> Result<Self, String> {
        let packages = v["packages"].as_array().ok_or("metadata: no packages")?;
        let name_of: BTreeMap<&str, &str> = packages
            .iter()
            .filter_map(|p| Some((p["id"].as_str()?, p["name"].as_str()?)))
            .collect();
        let nodes = v["resolve"]["nodes"]
            .as_array()
            .ok_or("metadata: no resolve graph")?;
        let mut dirs: BTreeMap<String, Vec<PkgDir>> = BTreeMap::new();
        let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for p in packages {
            if let (Some(name), Some(forge)) =
                (p["name"].as_str(), p["metadata"]["forge"].as_object())
            {
                declared.entry(name.to_string()).or_default().extend(
                    forge
                        .iter()
                        .filter(|(_, v)| v.as_bool() == Some(true))
                        .map(|(k, _)| k.clone()),
                );
            }
            let (Some(name), Some(manifest)) = (p["name"].as_str(), p["manifest_path"].as_str())
            else {
                continue;
            };
            if let Some(dir) = Path::new(manifest).parent() {
                dirs.entry(name.to_string()).or_default().push(PkgDir {
                    dir: dir.to_path_buf(),
                    local: p["source"].is_null(),
                });
            }
        }
        let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut features: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for n in nodes {
            let Some(id) = n["id"].as_str() else { continue };
            let Some(&name) = name_of.get(id) else {
                continue;
            };
            features.entry(name.to_string()).or_default().extend(
                n["features"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(str::to_string)),
            );
            let entry = edges.entry(name.to_string()).or_default();
            for d in n["deps"].as_array().into_iter().flatten() {
                let normal = d["dep_kinds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| k["kind"].is_null() || k["kind"].as_str() == Some("normal"));
                if !normal {
                    continue;
                }
                if let Some(&dn) = d["pkg"].as_str().and_then(|p| name_of.get(p)) {
                    entry.insert(dn.to_string());
                }
            }
        }
        Ok(Self {
            edges,
            features,
            dirs,
            declared,
        })
    }

    /// Replace the (workspace-unified) features with those cargo enables when building
    /// `pkg` **alone** (`cargo tree -p <pkg>`, host platform) — what that package's own
    /// build links, not what the rest of the workspace happens to switch on. `extra` is
    /// passed to cargo (`--offline`, `--features ...`).
    pub fn with_features_of(
        mut self,
        root: &Path,
        pkg: &str,
        extra: &[&str],
    ) -> Result<Self, String> {
        let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args([
                "tree", "-p", pkg, "-e", "normal", "--prefix", "none", "-f", "{p}|{f}",
            ])
            .args(extra)
            .current_dir(root)
            .output()
            .map_err(|e| format!("cannot run cargo tree: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "cargo tree failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let mut features: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some((p, f)) = line.split_once('|') else {
                continue;
            };
            let Some(name) = p.split_whitespace().next() else {
                continue;
            };
            features
                .entry(name.to_string())
                .or_default()
                .extend(f.split(',').filter(|s| !s.is_empty()).map(str::to_string));
        }
        if !features.contains_key(pkg) {
            return Err(format!("cargo tree -p {pkg} did not list {pkg}"));
        }
        self.features = features;
        Ok(self)
    }

    /// The features cargo enabled on `pkg` (empty if unknown).
    pub fn features(&self, pkg: &str) -> BTreeSet<String> {
        self.features.get(pkg).cloned().unwrap_or_default()
    }

    /// Where every resolved version of `pkg` lives (empty if unknown or synthetic).
    pub fn dirs(&self, pkg: &str) -> &[PkgDir] {
        self.dirs.get(pkg).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every package that declares `key = true` in its `[package.metadata.forge]` table.
    pub fn declaring(&self, key: &str) -> BTreeSet<String> {
        self.declared
            .iter()
            .filter(|(_, keys)| keys.contains(key))
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// The direct normal dependencies of `pkg`.
    pub fn deps(&self, pkg: &str) -> BTreeSet<String> {
        self.edges.get(pkg).cloned().unwrap_or_default()
    }

    /// Every package `root_pkg` links, transitively (excluding itself).
    /// `Err` if `root_pkg` is not in the graph — a guard asking about a crate that does not
    /// exist must fail, never pass vacuously (W2).
    pub fn closure(&self, root_pkg: &str) -> Result<BTreeSet<String>, String> {
        if !self.edges.contains_key(root_pkg) {
            return Err(format!("package {root_pkg} is not in the resolved graph"));
        }
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<&str> = VecDeque::from([root_pkg]);
        while let Some(p) = queue.pop_front() {
            for d in self.edges.get(p).into_iter().flatten() {
                if seen.insert(d.clone()) {
                    queue.push_back(d);
                }
            }
        }
        Ok(seen)
    }
}

fn host_triple() -> Result<String, String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|e| format!("cannot run rustc -vV: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .ok_or_else(|| "rustc -vV printed no host".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic() -> serde_json::Value {
        serde_json::json!({
            "packages": [
                {"id": "a", "name": "runtime"},
                {"id": "b", "name": "core"},
                {"id": "c", "name": "net"},
                {"id": "d", "name": "testutil"}
            ],
            "resolve": {"nodes": [
                {"id": "a", "deps": [
                    {"pkg": "b", "dep_kinds": [{"kind": null}]},
                    {"pkg": "d", "dep_kinds": [{"kind": "dev"}]}
                ]},
                {"id": "b", "deps": [{"pkg": "c", "dep_kinds": [{"kind": null}]}]},
                {"id": "c", "deps": []},
                {"id": "d", "deps": []}
            ]}
        })
    }

    #[test]
    fn closure_is_transitive_and_skips_dev_deps() {
        let g = DepGraph::from_metadata(&synthetic()).unwrap();
        let c = g.closure("runtime").unwrap();
        assert!(c.contains("core") && c.contains("net"), "{c:?}");
        assert!(
            !c.contains("testutil"),
            "dev-dependencies are not linked: {c:?}"
        );
    }

    #[test]
    fn positive_control_unknown_package_is_an_error_not_an_empty_set() {
        let g = DepGraph::from_metadata(&synthetic()).unwrap();
        assert!(g.closure("forge-runtime-that-does-not-exist").is_err());
    }

    #[test]
    fn resolves_the_real_workspace() {
        let g = DepGraph::resolve(&workspace_root()).unwrap();
        let c = g.closure("xtask").unwrap();
        assert!(
            c.contains("ron") && c.contains("serde") && c.contains("toml"),
            "{c:?}"
        );
        assert!(
            !c.contains("forge-runtime"),
            "xtask does not link forge-runtime: {c:?}"
        );
    }
}
