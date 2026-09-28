//! `test_2d_tax_is_zero` — **the 2D tax is zero** (Ch.31 §31.5, Ch.35 §35.3; I15; DoD
//! M4-11).
//!
//! A 2D project loads nothing a 3D world needs: no crate that declares itself 3D-only
//! (`[package.metadata.forge] not-in-2d = true` — the sky and atmosphere, `forge-sky`, and
//! every such crate the workspace has; the set is read from the workspace, never listed
//! here). Asserted against the **resolved plugin manifest**, not a comment: the 2D preset's
//! default plugin set as the built-in preset plugin ships it (`presets/2d/workspace.ron`
//! through `forge_editor::presets`), each plugin id resolved to the crate whose
//! `SourcePlugin` declares it (by its real manifest, so the table cannot drift), and each
//! crate's **resolved dependency closure** (`cargo metadata`, the build a user gets) checked
//! for the 3D-only crates. `forge-2d` itself is checked the same way, and a 2D world is one
//! frame deep: the 2D pipeline refuses a position in any other frame (`TWOD-0002`).
//!
//! Positive controls (W2): `positive_control_a_2d_preset_naming_a_3d_plugin_fails` (the real
//! 2D plugin list plus a plugin provided by a 3D-only crate), and
//! `positive_control_a_2d_crate_linking_a_3d_crate_fails` (a synthetic resolved graph in
//! which `forge-2d` reaches `forge-sky` through `forge-core`).
//!
//! **Scope (ADR 0047 amendment).** This checks the resolved plugin manifest and dependency
//! closure of what a 2D *project* loads and ships — the 2D preset's plugin set and its
//! crates' `cargo metadata` closure — never `tools/forge-editor-bin` itself. The one shared
//! editor binary loads every installed plugin unconditionally (exactly as it already does
//! `forge.2d`, `forge.importers` and `forge.presets`) so a person can switch presets in one
//! running editor; that is an editor-tool convenience, not a cost the 2D *runtime* a project
//! ships pays.

use std::collections::{BTreeMap, BTreeSet};

use forge_plugin::SourcePlugin;
use forge_tests::{DepGraph, workspace_root};

/// The `[package.metadata.forge]` key a crate sets to `true` to say a 2D project never links
/// it.
const NOT_IN_2D: &str = "not-in-2d";

/// The crates a 2D project must not link: every crate declaring [`NOT_IN_2D`].
fn not_in_2d(graph: &DepGraph) -> BTreeSet<String> {
    graph.declaring(NOT_IN_2D)
}

/// Every first-party plugin that can appear in a preset's plugin set: its manifest id (read
/// from the plugin itself) and the crate that provides it.
fn providers() -> BTreeMap<String, &'static str> {
    fn ok<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
        r.unwrap_or_else(|e| panic!("{e}"))
    }
    let plugins: Vec<(Box<dyn SourcePlugin>, &'static str)> = vec![
        (
            Box::new(ok(forge_editor::actions::EditorActions::new())),
            "forge-editor",
        ),
        (
            Box::new(ok(forge_panels_core::PanelsCore::new())),
            "forge-panels-core",
        ),
        (
            Box::new(ok(forge_panels_scene::PanelsScene::new())),
            "forge-panels-scene",
        ),
        (
            Box::new(ok(forge_panels_assets::PanelsAssets::new())),
            "forge-panels-assets",
        ),
        (
            Box::new(ok(forge_panels_domain::PanelsDomain::new())),
            "forge-panels-domain",
        ),
        (
            Box::new(ok(forge_panels_project::PanelsProject::new())),
            "forge-panels-project",
        ),
        (
            Box::new(ok(forge_importers::Importers::new())),
            "forge-importers",
        ),
        (
            Box::new(ok(forge_presets::BuiltinPresets::new())),
            "forge-presets",
        ),
        (
            Box::new(ok(forge_store_backends::StoreBackends::new())),
            "forge-store-backends",
        ),
        (Box::new(ok(forge_2d::Plugin2d::new())), "forge-2d"),
    ];
    plugins
        .into_iter()
        .map(|(pl, krate)| (pl.manifest().id.to_string(), krate))
        .collect()
}

/// What makes a 2D project pay for a 3D world (empty: nothing).
fn two_d_cost(
    plugins: &[String],
    providers: &BTreeMap<String, &'static str>,
    graph: &DepGraph,
) -> Vec<String> {
    let mut out = Vec::new();
    let banned = not_in_2d(graph);
    let mut crates: Vec<&str> = vec!["forge-2d"];
    for id in plugins {
        match providers.get(id) {
            Some(c) => crates.push(c),
            None => out.push(format!(
                "the 2D preset names {id}, which no known crate provides"
            )),
        }
    }
    crates.sort_unstable();
    crates.dedup();
    for c in crates {
        match graph.closure(c) {
            Ok(closure) => {
                for u in &banned {
                    if closure.contains(u) || c == u {
                        out.push(format!("{c} (in a 2D project) links {u}"));
                    }
                }
            }
            Err(e) => out.push(e),
        }
    }
    out
}

fn the_2d_plugins() -> Vec<String> {
    forge_editor::presets::builtin_preset("2d")
        .unwrap_or_else(|e| panic!("{e}"))
        .workspace
        .plugins
}

#[test]
fn test_2d_tax_is_zero() {
    let plugins = the_2d_plugins();
    assert!(
        plugins.iter().any(|p| p == "forge.2d"),
        "the 2D preset enables the 2D pipeline: {plugins:?}"
    );
    let graph = DepGraph::resolve(&workspace_root()).unwrap_or_else(|e| panic!("{e}"));
    // The set is not empty: the sky and atmosphere are 3D-only.
    assert!(
        not_in_2d(&graph).contains("forge-sky"),
        "forge-sky declares {NOT_IN_2D}: {:?}",
        not_in_2d(&graph)
    );
    let cost = two_d_cost(&plugins, &providers(), &graph);
    assert!(
        cost.is_empty(),
        "a 2D project pays for a 3D world:\n{}",
        cost.join("\n")
    );
}

#[test]
fn a_2d_world_is_one_frame_deep() {
    use forge_2d::physics::{BodyDef, BodyKind, PhysicsWorld2d};
    use forge_2d::{DVec2, FrameId, FramePos2};
    let mut w = PhysicsWorld2d::new(FrameId(0));
    assert!(
        w.add_body(&BodyDef::new(
            BodyKind::Dynamic,
            FramePos2::new(FrameId(0), DVec2::ZERO)
        ))
        .is_ok()
    );
    let e = w
        .add_body(&BodyDef::new(
            BodyKind::Dynamic,
            FramePos2::new(FrameId(1), DVec2::ZERO),
        ))
        .expect_err("a second frame");
    assert_eq!(e.code().as_str(), "TWOD-0002");
}

#[test]
fn positive_control_a_2d_preset_naming_a_3d_plugin_fails() {
    let mut plugins = the_2d_plugins();
    plugins.push("com.example.sky".into());
    let mut providers = providers();
    providers.insert("com.example.sky".into(), "forge-sky");
    let graph = DepGraph::resolve(&workspace_root()).unwrap_or_else(|e| panic!("{e}"));
    let cost = two_d_cost(&plugins, &providers, &graph);
    assert!(
        cost.iter().any(|c| c.contains("links forge-sky")),
        "{cost:?}"
    );
    // A plugin id no crate provides is named too.
    let mut plugins = the_2d_plugins();
    plugins.push("com.example.unknown".into());
    let cost = two_d_cost(&plugins, &self::providers(), &graph);
    assert!(
        cost.iter().any(|c| c.contains("com.example.unknown")),
        "{cost:?}"
    );
}

#[test]
fn positive_control_a_2d_crate_linking_a_3d_crate_fails() {
    let meta = serde_json::json!({
        "packages": [
            {"id": "2d", "name": "forge-2d"},
            {"id": "core", "name": "forge-core"},
            {"id": "sky", "name": "forge-sky", "metadata": {"forge": {"not-in-2d": true}}}
        ],
        "resolve": {"nodes": [
            {"id": "2d", "deps": [{"pkg": "core", "dep_kinds": [{"kind": null}]}]},
            {"id": "core", "deps": [{"pkg": "sky", "dep_kinds": [{"kind": null}]}]},
            {"id": "sky", "deps": []}
        ]}
    });
    let graph = DepGraph::from_metadata(&meta).unwrap_or_else(|e| panic!("{e}"));
    let cost = two_d_cost(&[], &providers(), &graph);
    assert!(
        cost.iter()
            .any(|c| c.contains("forge-2d") && c.contains("forge-sky")),
        "{cost:?}"
    );
}

// ---- the exported 2D game (WP-U16, M4-12 / M4-13) -----------------------------------------
//
// The 2D sample game (`samples/2d-game`) is exported the way a customer's game is: the
// `forge_project::packager::CargoPackager` builds its binary in release, writes the package
// folder and keeps the linker's map. Two independent witnesses must agree that **the shipped
// binary links no 3D-only crate** (and no editor crate):
//
// * the build's dependency closure as cargo resolves it for exactly the exported features
//   (`cargo tree -e normal`), and
// * the **link map** of the executable: every object file and symbol the linker put in it —
//   a crate that is linked names itself there (`forge_sky-<hash>.rlib`, `_ZN9forge_sky`).
//
// The exported binary then plays the scripted run headless through its own menus to the
// level-clear banner and must print the pinned win state (`forge_2d_game::SCRIPT_WIN`): the
// release build is bit-identical to the debug build and, under `just verify-linux`, to the
// Windows one. Its size and headless startup are recorded.
//
// Positive control (W2): `positive_control_an_export_linking_the_sky_fails` exports the
// same game with `control-link-3d` (the binary calls `forge_sky` for real) and both witnesses
// must name `forge-sky`.

/// Editor crates: a shipped game links none of them either.
const EDITOR_CRATES: &[&str] = &[
    "forge-editor",
    "forge-panels-core",
    "forge-panels-scene",
    "forge-panels-assets",
    "forge-panels-domain",
    "forge-panels-project",
    "forge-panels-authoring",
];

struct Exported {
    report: forge_project::packager::BuildReport,
    exe: std::path::PathBuf,
    /// The crates the build links, from `cargo tree`.
    tree: std::collections::BTreeSet<String>,
    /// The link map's text.
    map: String,
}

/// Export the 2D sample with `features` into its own package folder (serialised across test
/// processes by a file lock: the variants share one release target directory).
fn export(features: &[&str]) -> Exported {
    use forge_project::packager::{CargoPackager, Packager, Product};
    let root = workspace_root();
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("export-2d");
    std::fs::create_dir_all(&base).unwrap_or_else(|e| panic!("{e}"));
    let lock = std::fs::File::create(base.join("export.lock")).unwrap_or_else(|e| panic!("{e}"));
    lock.lock().unwrap_or_else(|e| panic!("{e}"));
    let variant = if features.is_empty() {
        "shipped".to_string()
    } else {
        features.join("+")
    };
    let mut p = CargoPackager::new(
        &root,
        "forge-2d-game",
        "forge-2d-game",
        base.join("build"),
        base.join(&variant),
    );
    p.features = features.iter().map(|f| (*f).to_string()).collect();
    let target = CargoPackager::host_target().unwrap_or_else(|| panic!("no desktop target"));
    let report = p
        .package(
            target,
            &Product {
                name: "Lantern Run".into(),
                version: "0.1.0".into(),
                gpu_mode: "single".into(),
            },
            &[],
        )
        .unwrap_or_else(|e| panic!("the export failed: {e}"));
    let exe = base.join(&variant).join(&report.files[0].path);
    let map_path = report
        .link_map
        .clone()
        .unwrap_or_else(|| panic!("no link map"));
    let map = String::from_utf8_lossy(
        &std::fs::read(&map_path).unwrap_or_else(|e| panic!("{map_path}: {e}")),
    )
    .into_owned();
    let mut cmd =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(&root).args([
        "tree",
        "--locked",
        "--offline",
        "-p",
        "forge-2d-game",
        "-e",
        "normal",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ]);
    if !features.is_empty() {
        cmd.args(["--features", &features.join(",")]);
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("cargo tree: {e}"));
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_string)
        .collect();
    drop(lock);
    Exported {
        report,
        exe,
        tree,
        map,
    }
}

/// What the exported binary links that a 2D product must not: 3D-only crates (and editor
/// crates), by either witness.
fn export_cost(e: &Exported) -> Vec<String> {
    let graph = DepGraph::resolve(&workspace_root()).unwrap_or_else(|err| panic!("{err}"));
    let banned = not_in_2d(&graph);
    let mut out = Vec::new();
    for c in banned
        .iter()
        .map(String::as_str)
        .chain(EDITOR_CRATES.iter().copied())
    {
        if e.tree.contains(c) {
            out.push(format!("cargo tree: the exported build depends on {c}"));
        }
        // The linker names a linked crate by its ident: its rlib and its mangled symbols.
        if let Some(line) = map_links(&e.map, c) {
            out.push(format!(
                "link map: the exported binary links {c}: {}",
                line.trim()
            ));
        }
    }
    out
}

/// The first line of a link map showing crate `krate` linked, if any. A linked crate names
/// itself two ways: its object files (`forge_sky-<hash>.forge_sky.<cgu>.rcgu.o`, from
/// `libforge_sky-<hash>.rlib`) and its mangled symbols, where the crate ident is
/// length-prefixed (`9forge_sky`, legacy and v0 mangling alike). Both are matched
/// exactly: `forge_2d` does not match `forge_2d_game` (`8forge_2d` is not `13forge_2d_game`,
/// and the object name needs the `-<hash>` right after the ident).
fn map_links<'m>(map: &'m str, krate: &str) -> Option<&'m str> {
    let ident = krate.replace('-', "_");
    let object = format!("{ident}-");
    let mangled = format!("{}{ident}", ident.len());
    map.lines().find(|line| {
        let starts = |pat: &str| {
            line.match_indices(pat).any(|(i, _)| {
                let before = line[..i].chars().next_back();
                before.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
                    || (pat == mangled && before.is_some_and(|c| !c.is_ascii_digit()))
            })
        };
        starts(&object) || starts(&mangled)
    })
}
#[test]
fn the_exported_2d_game_links_no_3d_crate() {
    let e = export(&[]);
    // The witnesses see the build at all (not vacuous): the 2D pipeline and the game-UI
    // runtime are in both.
    for c in ["forge-2d", "forge-runtime", "forge-ui"] {
        assert!(e.tree.contains(c), "cargo tree lists {c}: {:?}", e.tree);
        assert!(map_links(&e.map, c).is_some(), "the link map names {c}");
    }
    let cost = export_cost(&e);
    assert!(
        cost.is_empty(),
        "the exported 2D game pays for a 3D world:\n{}",
        cost.join("\n")
    );
    let names: Vec<&str> = e.report.files.iter().map(|f| f.path.as_str()).collect();
    for f in ["forge.json", "NOTICES", "credits.txt"] {
        assert!(names.contains(&f), "the package carries {f}: {names:?}");
    }
    // The shipped binary plays the scripted run through its menus to the win state.
    let out = std::process::Command::new(&e.exe)
        .arg("--play-script")
        .output()
        .unwrap_or_else(|err| panic!("{}: {err}", e.exe.display()));
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{}", text.trim());
    let (steps, fp) = forge_2d_game::SCRIPT_WIN;
    assert!(
        out.status.success(),
        "the exported game did not clear the level: {text}"
    );
    assert!(
        text.contains(&format!("won=true steps={steps} "))
            && text.contains(&format!("fingerprint={:#018x}", fp.0)),
        "the exported (release) build reaches the pinned win state bit for bit: {text}"
    );
    // Recorded: binary size and headless startup to the first rendered frame.
    let bytes = e.report.files[0].bytes;
    println!(
        "exported 2D game ({}): {} bytes ({:.1} MiB)",
        std::env::consts::OS,
        bytes,
        bytes as f64 / (1024.0 * 1024.0)
    );
    let probe = std::process::Command::new(&e.exe)
        .arg("--probe")
        .output()
        .unwrap_or_else(|err| panic!("{err}"));
    let ptext = String::from_utf8_lossy(&probe.stdout);
    if probe.status.success() {
        println!("exported 2D game startup: {}", ptext.trim());
    } else {
        let ci = std::env::var("CI").is_ok_and(|v| v == "true" || v == "1");
        let why = String::from_utf8_lossy(&probe.stderr);
        assert!(!ci, "CI=true and the exported game's probe failed: {why}");
        println!(
            "exported 2D game startup: AWAITING(no GPU adapter: {})",
            why.trim()
        );
    }
}

#[test]
fn positive_control_an_export_linking_the_sky_fails() {
    let e = export(&["control-link-3d"]);
    let cost = export_cost(&e);
    assert!(
        cost.iter()
            .any(|c| c.starts_with("cargo tree") && c.contains("forge-sky")),
        "cargo tree must see forge-sky in the control build: {cost:?}"
    );
    assert!(
        cost.iter()
            .any(|c| c.starts_with("link map") && c.contains("forge-sky")),
        "the link map must show forge-sky linked into the control binary: {cost:?}"
    );
}
