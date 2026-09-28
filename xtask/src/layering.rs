//! `cargo xtask layering` — **the host assembles plugins** (WP-21, ADR 0045, plan §32.8).
//!
//! The engine is a kernel (`crates/*`) plus plugins (`plugins/*`, I16). The dependency arrow
//! between them points one way — plugins build on the kernel — with one reasoned exception: a
//! **host**, a crate that assembles a plugin set through the ordinary loader, may depend on
//! the plugins it loads (the editor library's default set, the project host's store
//! backends). The rules, checked over every workspace member's `Cargo.toml`:
//!
//! * **L1.** A `crates/*` crate names a `plugins/*` crate in `[dependencies]` or
//!   `[build-dependencies]` (any target) only if [`HOSTS`] lists that exact edge. A kernel
//!   crate that is not a host would otherwise compile a plugin into everything that links it,
//!   and a plugin would stop being replaceable.
//! * **L2.** `[dev-dependencies]` are free: a test assembles plugins as a host does.
//! * **L3.** A `plugins/*` crate never depends on a `tools/*` crate (a binary or its
//!   library half): plugins sit below every host.
//! * **L4.** The allow-list is exact: an entry of [`HOSTS`] whose edge no longer exists is an
//!   error too, so the list can only say what is true.
//! * **L5.** The `controls` feature (W2 fault switches, ADR 0046) is turned on only by
//!   `[dev-dependencies]`: no `[dependencies]` or `[build-dependencies]` entry lists it,
//!   and no feature but `controls` itself enables it (`default = ["controls"]`,
//!   `gui = ["forge-ui/controls"]`). So a production build (the editor, a game, a release
//!   `forge`) never links a control branch.
//!
//! `tools/*`, `samples/*`, `xtask` and the `tests` crate may depend on anything (they are hosts and test
//! harnesses by definition).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::util::{Failure, XResult, read};

/// The kernel crates that are hosts, and the plugin crates each may assemble in its normal
/// dependencies, with why (L1, L4).
pub const HOSTS: &[(&str, &str, &str)] = &[
    (
        "forge-editor",
        "forge-importers",
        "the editor library's default plugin set: shell::assemble loads the first-party importers through the loader",
    ),
    (
        "forge-editor",
        "forge-presets",
        "the editor library's default plugin set: the built-in presets, loaded through the loader",
    ),
    (
        "forge-project",
        "forge-store-backends",
        "ProjectHost::first_party opens stores through the StoreBackend registry the first-party backends fill",
    ),
];

/// Where a workspace member lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// `crates/*`: the kernel.
    Kernel,
    /// `plugins/*`.
    Plugin,
    /// `tools/*`: binaries and their library halves.
    Tool,
    /// `xtask`, `tests`: automation and harnesses.
    Other,
}

/// Which dependency table an edge is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Normal,
    Build,
    Dev,
}

impl Kind {
    fn table(self) -> &'static str {
        match self {
            Self::Normal => "dependencies",
            Self::Build => "build-dependencies",
            Self::Dev => "dev-dependencies",
        }
    }
}

/// One workspace member and its path dependencies on other members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub layer: Layer,
    /// The manifest, for messages.
    pub manifest: String,
    /// `(dependency crate, its layer, table)`.
    pub deps: Vec<(String, Layer, Kind)>,
    /// Where a non-dev edge or a feature other than `controls` turns `controls` on (L5).
    pub controls_leaks: Vec<String>,
}

/// Check the rules over `members` (see the module docs). Returns every violation.
#[must_use]
pub fn check(members: &[Member], hosts: &[(&str, &str, &str)]) -> Vec<String> {
    let mut errs = Vec::new();
    let mut used: BTreeSet<(String, String)> = BTreeSet::new();
    for m in members {
        for (dep, layer, kind) in &m.deps {
            match (m.layer, *layer, *kind) {
                (Layer::Kernel, Layer::Plugin, Kind::Normal | Kind::Build) => {
                    if hosts.iter().any(|(h, p, _)| *h == m.name && p == dep) {
                        used.insert((m.name.clone(), dep.clone()));
                    } else {
                        errs.push(format!(
                            "{}: kernel crate {} depends on plugin {dep} in [{}] (L1): only a host assembles plugins — load it through the loader from a host (tools/*, or a crate listed in xtask/src/layering.rs HOSTS with its reason), or make it a [dev-dependencies] entry for tests",
                            m.manifest,
                            m.name,
                            kind.table()
                        ));
                    }
                }
                (Layer::Plugin, Layer::Tool, _) => errs.push(format!(
                    "{}: plugin {} depends on {dep}, a tools/* crate, in [{}] (L3): plugins sit below every host",
                    m.manifest,
                    m.name,
                    kind.table()
                )),
                _ => {}
            }
        }
    }
    for m in members {
        for leak in &m.controls_leaks {
            errs.push(format!(
                "{}: {leak} turns on the `controls` feature outside [dev-dependencies] (L5): W2 fault switches must never reach a production build — enable it from [dev-dependencies] only",
                m.manifest
            ));
        }
    }
    for (h, p, _) in hosts {
        if !used.contains(&((*h).to_string(), (*p).to_string())) {
            errs.push(format!(
                "xtask/src/layering.rs: HOSTS allows {h} -> {p}, but {h} does not depend on {p} (L4): remove the stale allowance"
            ));
        }
    }
    errs
}

fn layer_of(rel: &Path) -> Option<(Layer, String)> {
    let mut c = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    let first = c.next()?;
    let second = c.next();
    match (first.as_str(), second) {
        ("crates", Some(n)) => Some((Layer::Kernel, n)),
        ("plugins", Some(n)) => Some((Layer::Plugin, n)),
        ("tools" | "samples", Some(n)) => Some((Layer::Tool, n)),
        ("xtask", None) | ("tests", None) | ("tests-premium", None) => Some((Layer::Other, first)),
        _ => None,
    }
}

/// Resolve `a/../b` segments lexically (no filesystem access: the target may be a member
/// that is not built on this machine).
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The path dependencies of one manifest's tables (top level and every `[target.*]`).
fn path_deps(
    doc: &toml::Table,
    dir: &Path,
    workspace_paths: &BTreeMap<String, PathBuf>,
) -> Vec<(String, PathBuf, Kind)> {
    let mut tables: Vec<(&toml::Table, Kind)> = Vec::new();
    fn push<'a>(t: &'a toml::Table, tables: &mut Vec<(&'a toml::Table, Kind)>) {
        for (key, kind) in [
            ("dependencies", Kind::Normal),
            ("build-dependencies", Kind::Build),
            ("dev-dependencies", Kind::Dev),
        ] {
            if let Some(toml::Value::Table(d)) = t.get(key) {
                tables.push((d, kind));
            }
        }
    }
    push(doc, &mut tables);
    let mut targets = Vec::new();
    if let Some(toml::Value::Table(t)) = doc.get("target") {
        for v in t.values() {
            if let toml::Value::Table(tt) = v {
                targets.push(tt);
            }
        }
    }
    for tt in targets {
        push(tt, &mut tables);
    }
    let mut out = Vec::new();
    for (t, kind) in tables {
        for (name, spec) in t {
            let toml::Value::Table(spec) = spec else {
                continue;
            };
            let crate_name = spec
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(name)
                .to_string();
            if let Some(p) = spec.get("path").and_then(toml::Value::as_str) {
                out.push((crate_name, normalise(&dir.join(p)), kind));
            } else if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true)
                && let Some(p) = workspace_paths.get(name)
            {
                out.push((crate_name, p.clone(), kind));
            }
        }
    }
    out
}

/// Every place in one manifest that turns `controls` on outside `[dev-dependencies]` (L5):
/// a `[dependencies]` / `[build-dependencies]` entry (top level or `[target.*]`) whose
/// `features` list it, and a feature other than `controls` that enables `controls` or
/// `<dep>/controls`.
fn controls_leaks(doc: &toml::Table) -> Vec<String> {
    let is_controls = |s: &str| s == "controls" || s.ends_with("/controls");
    let mut out = Vec::new();
    let mut tables: Vec<(&toml::Table, String)> = Vec::new();
    for key in ["dependencies", "build-dependencies"] {
        if let Some(toml::Value::Table(d)) = doc.get(key) {
            tables.push((d, format!("[{key}]")));
        }
    }
    if let Some(toml::Value::Table(t)) = doc.get("target") {
        for (cfg, v) in t {
            for key in ["dependencies", "build-dependencies"] {
                if let Some(toml::Value::Table(d)) = v.get(key) {
                    tables.push((d, format!("[target.{cfg}.{key}]")));
                }
            }
        }
    }
    for (t, where_) in tables {
        for (name, spec) in t {
            let on = spec
                .get("features")
                .and_then(toml::Value::as_array)
                .is_some_and(|f| f.iter().filter_map(toml::Value::as_str).any(is_controls));
            if on {
                out.push(format!("{where_} {name}"));
            }
        }
    }
    if let Some(toml::Value::Table(fs)) = doc.get("features") {
        for (f, list) in fs {
            if f == "controls" {
                continue;
            }
            let on = list
                .as_array()
                .is_some_and(|l| l.iter().filter_map(toml::Value::as_str).any(is_controls));
            if on {
                out.push(format!("[features] {f}"));
            }
        }
    }
    out
}

/// Read every workspace member under `root`.
pub fn members(root: &Path) -> XResult<Vec<Member>> {
    let parse = |p: &Path| -> XResult<toml::Table> {
        read(p)?
            .parse::<toml::Table>()
            .map_err(|e| Failure::one(format!("{}: {e}", p.display())))
    };
    let root_doc = parse(&root.join("Cargo.toml"))?;
    let mut workspace_paths = BTreeMap::new();
    if let Some(toml::Value::Table(ws)) = root_doc.get("workspace")
        && let Some(toml::Value::Table(deps)) = ws.get("dependencies")
    {
        for (k, v) in deps {
            if let Some(p) = v.get("path").and_then(toml::Value::as_str) {
                workspace_paths.insert(k.clone(), normalise(&root.join(p)));
            }
        }
    }
    // The workspace's own `members` list: a `group/*` glob (D-6) or a directory.
    let members: Vec<String> = root_doc
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_else(|| {
            [
                "crates/*",
                "plugins/*",
                "tools/*",
                "samples/*",
                "xtask",
                "tests",
            ]
            .map(str::to_string)
            .to_vec()
        });
    let mut dirs: Vec<PathBuf> = Vec::new();
    for m in &members {
        if let Some(group) = m.strip_suffix("/*") {
            let Ok(rd) = std::fs::read_dir(root.join(group)) else {
                continue;
            };
            for e in rd.filter_map(Result::ok) {
                if e.path().join("Cargo.toml").is_file() {
                    dirs.push(e.path());
                }
            }
        } else if root.join(m).join("Cargo.toml").is_file() {
            dirs.push(root.join(m));
        }
    }
    dirs.sort();
    let root_n = normalise(root);
    let mut out = Vec::new();
    for dir in dirs {
        let manifest = dir.join("Cargo.toml");
        let doc = parse(&manifest)?;
        let rel = normalise(&dir);
        let rel = rel.strip_prefix(&root_n).unwrap_or(&rel).to_path_buf();
        let Some((layer, _)) = layer_of(&rel) else {
            continue;
        };
        let name = doc
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let controls_leaks = controls_leaks(&doc);
        let mut deps = Vec::new();
        for (dep, path, kind) in path_deps(&doc, &normalise(&dir), &workspace_paths) {
            let r = path.strip_prefix(&root_n).unwrap_or(&path).to_path_buf();
            if let Some((l, _)) = layer_of(&r) {
                deps.push((dep, l, kind));
            }
        }
        out.push(Member {
            name,
            layer,
            manifest: rel.join("Cargo.toml").to_string_lossy().replace('\\', "/"),
            deps,
            controls_leaks,
        });
    }
    Ok(out)
}

/// How many members declare a `controls` feature (the run's summary line).
fn controls_declared(root: &Path) -> usize {
    ["crates", "plugins", "tools", "samples"]
        .iter()
        .filter_map(|g| std::fs::read_dir(root.join(g)).ok())
        .flat_map(|rd| rd.filter_map(Result::ok))
        .filter_map(|e| std::fs::read_to_string(e.path().join("Cargo.toml")).ok())
        .filter(|t| {
            t.lines()
                .any(|l| l.trim_start().starts_with("controls = ["))
        })
        .count()
}

/// `cargo xtask layering`.
pub fn run(root: &Path) -> XResult<String> {
    let ms = members(root)?;
    Failure::many(check(&ms, HOSTS))?;
    let kernel_to_plugin = ms
        .iter()
        .filter(|m| m.layer == Layer::Kernel)
        .flat_map(|m| m.deps.iter().filter(|(_, l, _)| *l == Layer::Plugin))
        .count();
    Ok(format!(
        "layering: {} members; host-assembles-plugins holds ({} kernel->plugin edges: {} allowed host edges, the rest dev-dependencies); {} crates declare W2 `controls`, none turned on outside [dev-dependencies]\n",
        ms.len(),
        kernel_to_plugin,
        HOSTS.len(),
        controls_declared(root)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, layer: Layer, deps: &[(&str, Layer, Kind)]) -> Member {
        Member {
            name: name.into(),
            layer,
            manifest: format!("{name}/Cargo.toml"),
            deps: deps
                .iter()
                .map(|(d, l, k)| ((*d).to_string(), *l, *k))
                .collect(),
            controls_leaks: Vec::new(),
        }
    }

    #[test]
    fn the_repository_holds_the_rule() {
        let ms = members(&crate::util::workspace_root()).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            ms.iter().any(|m| m.name == "forge-editor"),
            "members are read"
        );
        let errs = check(&ms, HOSTS);
        assert!(errs.is_empty(), "{errs:#?}");
    }

    #[test]
    fn positive_control_each_layering_break_is_caught() {
        let host = [("forge-editor", "forge-presets", "why")];
        let good = vec![
            member(
                "forge-editor",
                Layer::Kernel,
                &[("forge-presets", Layer::Plugin, Kind::Normal)],
            ),
            member(
                "forge-asset",
                Layer::Kernel,
                &[("forge-importers", Layer::Plugin, Kind::Dev)],
            ),
            member(
                "forge-presets",
                Layer::Plugin,
                &[("forge-plugin", Layer::Kernel, Kind::Normal)],
            ),
        ];
        assert!(check(&good, &host).is_empty(), "the faithful model passes");
        // L1: a kernel crate that is not a host, in [dependencies] and in [build-dependencies].
        for kind in [Kind::Normal, Kind::Build] {
            let mut bad = good.clone();
            bad[1]
                .deps
                .push(("forge-importers".into(), Layer::Plugin, kind));
            let errs = check(&bad, &host);
            assert!(
                errs.len() == 1
                    && errs[0].contains("forge-asset depends on plugin forge-importers")
                    && errs[0].contains("(L1)"),
                "{errs:?}"
            );
        }
        // L1: a host reaching a plugin its entry does not name.
        let mut bad = good.clone();
        bad[0]
            .deps
            .push(("forge-importers".into(), Layer::Plugin, Kind::Normal));
        assert_eq!(check(&bad, &host).len(), 1);
        // L3: a plugin on a tool.
        let mut bad = good.clone();
        bad[2]
            .deps
            .push(("forge-editor-bin".into(), Layer::Tool, Kind::Dev));
        let errs = check(&bad, &host);
        assert!(errs.len() == 1 && errs[0].contains("(L3)"), "{errs:?}");
        // L4: a stale allowance.
        let mut bad = good.clone();
        bad[0].deps.clear();
        let errs = check(&bad, &host);
        assert!(errs.len() == 1 && errs[0].contains("(L4)"), "{errs:?}");
    }

    #[test]
    fn positive_control_controls_outside_dev_dependencies_is_caught() {
        let parse = |s: &str| s.parse::<toml::Table>().unwrap_or_else(|e| panic!("{e}"));
        // The faithful shape: a crate declares `controls`, forwards it, and turns it on for
        // its own tests through a [dev-dependencies] self edge.
        let good = parse(
            "[features]\ncontrols = [\"forge-ui/controls\"]\ngui = [\"forge-ui/winit\"]\n[dependencies]\nforge-ui = { path = \"../forge-ui\" }\n[dev-dependencies]\nforge-editor = { path = \".\", features = [\"controls\"] }\n",
        );
        assert!(
            controls_leaks(&good).is_empty(),
            "{:?}",
            controls_leaks(&good)
        );
        for (bad, what) in [
            (
                "[dependencies]\nforge-ui = { path = \"../forge-ui\", features = [\"controls\"] }\n",
                "[dependencies] forge-ui",
            ),
            (
                "[target.'cfg(windows)'.dependencies]\nforge-ui = { path = \"../forge-ui\", features = [\"controls\"] }\n",
                "[target.cfg(windows).dependencies] forge-ui",
            ),
            (
                "[build-dependencies]\nforge-ui = { path = \"../forge-ui\", features = [\"forge-ui/controls\"] }\n",
                "[build-dependencies] forge-ui",
            ),
            (
                "[features]\ndefault = [\"controls\"]\ncontrols = []\n",
                "[features] default",
            ),
            (
                "[features]\ngui = [\"forge-ui?/controls\"]\n",
                "[features] gui",
            ),
        ] {
            let leaks = controls_leaks(&parse(bad));
            assert_eq!(leaks, vec![what.to_string()], "{bad}");
            let mut m = member("forge-editor", Layer::Kernel, &[]);
            m.controls_leaks = leaks;
            let errs = check(&[m], &[]);
            assert!(errs.len() == 1 && errs[0].contains("(L5)"), "{errs:?}");
        }
    }

    #[test]
    fn target_specific_and_workspace_path_dependencies_are_read() {
        let dir = std::env::temp_dir().join(format!("forge-xtask-layering-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |p: &str, t: &str| {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap_or(&dir)).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(f, t).unwrap_or_else(|e| panic!("{e}"));
        };
        write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"tools/*\", \"plugins/*\"]\n[workspace.dependencies]\nforge-presets = { path = \"plugins/forge-presets\" }\n",
        );
        write(
            "crates/forge-cmd/Cargo.toml",
            "[package]\nname = \"forge-cmd\"\n[target.'cfg(windows)'.dependencies]\nforge-importers = { path = \"../../plugins/forge-importers\" }\n[dependencies]\nforge-presets = { workspace = true }\n",
        );
        write(
            "plugins/forge-presets/Cargo.toml",
            "[package]\nname = \"forge-presets\"\n",
        );
        write(
            "plugins/forge-importers/Cargo.toml",
            "[package]\nname = \"forge-importers\"\n[dev-dependencies]\nforge-editor-bin = { path = \"../../tools/forge-editor-bin\" }\n",
        );
        let ms = members(&dir).unwrap_or_else(|e| panic!("{e}"));
        let errs = check(&ms, &[]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(errs.len(), 3, "{errs:#?}");
        assert!(
            errs.iter()
                .any(|e| e.contains("forge-cmd depends on plugin forge-importers"))
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("forge-cmd depends on plugin forge-presets"))
        );
        assert!(errs.iter().any(|e| e.contains("(L3)")));
    }
}
