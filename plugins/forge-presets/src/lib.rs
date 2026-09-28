//! `forge-presets` — the built-in workspace presets as a plugin (Ch.31, M2-14).
//!
//! `forge.presets` provides `Preset("forge.preset.2d")` and `Preset("forge.preset.3d")`: the
//! repository's `presets/` directories (compiled in, so they always load), each as a
//! [`PresetDescriptor`] carrying its label, family, default settings and **all of its
//! files**. The editor builds its workspace from the `Preset`
//! registry, so a third-party preset plugin sits beside these on equal terms, and a project
//! can replace or remove a built-in one (I15, I16).
//!
//! ```
//! use forge_plugin::points::Preset;
//! use forge_plugin::{Extensions, Grants, loader};
//!
//! let mut x = Extensions::new();
//! x.define::<Preset>()?;
//! loader::load(&mut x, &[&forge_presets::BuiltinPresets::new()?], &[], &Grants::new())?;
//! let three = x.registry::<Preset>().and_then(|r| r.get("forge.preset.3d")).expect("3D");
//! assert_eq!(three.label, "3D");
//! assert!(three.files.contains_key("layout.ron"));
//! # Ok::<(), forge_plugin::PluginError>(())
//! ```

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use forge_plugin::points::{Preset, PresetDescriptor, PresetKind};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use serde::Deserialize;

const MANIFEST: &str = r#"Plugin(
    id: "forge.presets",
    version: "0.1.0",
    engine: "^0.1",
    kind: Source,
    provides: [
        Preset("forge.preset.2d"), Preset("forge.preset.3d"),
    ],
)"#;

/// `(preset dir, file, text)` of every built-in preset file.
const FILES: &[(&str, &str, &str)] = &[
    (
        "2d",
        "workspace.ron",
        include_str!("../../../presets/2d/workspace.ron"),
    ),
    (
        "2d",
        "layout.ron",
        include_str!("../../../presets/2d/layout.ron"),
    ),
    (
        "2d",
        "keymap.ron",
        include_str!("../../../presets/2d/keymap.ron"),
    ),
    (
        "2d",
        "templates/2d_empty.scene.ron",
        include_str!("../../../presets/2d/templates/2d_empty.scene.ron"),
    ),
    (
        "3d",
        "workspace.ron",
        include_str!("../../../presets/3d/workspace.ron"),
    ),
    (
        "3d",
        "layout.ron",
        include_str!("../../../presets/3d/layout.ron"),
    ),
    (
        "3d",
        "templates/3d_empty.scene.ron",
        include_str!("../../../presets/3d/templates/3d_empty.scene.ron"),
    ),
];

/// The built-in preset directories, in the order the new-project dialog lists them.
pub const DIRS: &[&str] = &["2d", "3d"];

/// The registry key of the built-in preset in `dir` (`forge.preset.3d`).
#[must_use]
pub fn key_of(dir: &str) -> String {
    format!("forge.preset.{dir}")
}

/// The part of `workspace.ron` a descriptor summarises (the editor reads the whole file).
#[derive(Deserialize)]
struct Summary {
    id: String,
    label: String,
    family: Family,
    #[serde(default)]
    settings: BTreeMap<String, String>,
}

#[derive(Deserialize)]
enum Family {
    TwoD,
    ThreeD,
}

/// The descriptor of the built-in preset in `dir`.
pub fn descriptor(dir: &str) -> Result<PresetDescriptor, PluginError> {
    let bad = |why: String| PluginError::Manifest {
        plugin: Some("forge.presets".into()),
        why: format!("presets/{dir}: {why}"),
    };
    let files: BTreeMap<String, String> = FILES
        .iter()
        .filter(|(d, _, _)| *d == dir)
        .map(|(_, f, t)| ((*f).to_string(), (*t).to_string()))
        .collect();
    let text = files
        .get("workspace.ron")
        .ok_or_else(|| bad("no workspace.ron".into()))?;
    let s: Summary = ron::from_str(text).map_err(|e| bad(e.to_string()))?;
    if s.id != key_of(dir) {
        return Err(bad(format!("its id is {:?}, not {:?}", s.id, key_of(dir))));
    }
    Ok(PresetDescriptor {
        label: s.label,
        kind: match s.family {
            Family::TwoD => PresetKind::TwoD,
            Family::ThreeD => PresetKind::ThreeD,
        },
        defaults: s.settings,
        files,
    })
}

/// The built-in presets.
pub struct BuiltinPresets {
    manifest: Manifest,
}

impl BuiltinPresets {
    /// The plugin.
    pub fn new() -> Result<Self, PluginError> {
        Ok(Self {
            manifest: Manifest::parse(MANIFEST)?,
        })
    }
}

impl SourcePlugin for BuiltinPresets {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for dir in DIRS {
            cx.add::<Preset>(&key_of(dir), descriptor(dir)?, Order::Last)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files under `dir`, recursively (a preset's `templates/` too).
    fn files_under(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |rd| {
            rd.filter_map(Result::ok)
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() { files_under(&p) } else { 1 }
                })
                .sum()
        })
    }

    #[test]
    fn every_preset_directory_on_disk_is_provided() {
        let on_disk: Vec<String> =
            std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../presets"))
                .map(|rd| {
                    rd.filter_map(Result::ok)
                        .filter(|e| e.path().is_dir())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
        let mut on_disk = on_disk;
        on_disk.sort();
        assert_eq!(on_disk, DIRS, "a preset directory is not compiled in");
        for dir in DIRS {
            let d = descriptor(dir).unwrap_or_else(|e| panic!("{e}"));
            let disk = files_under(&std::path::PathBuf::from(format!(
                "{}/../../presets/{dir}",
                env!("CARGO_MANIFEST_DIR")
            )));
            assert_eq!(
                d.files.len(),
                disk,
                "presets/{dir}: a file is not compiled in"
            );
        }
    }
}
