//! `cargo xtask fmt [--check]`: rustfmt over every workspace package, a group at a time.
//!
//! `cargo fmt --all` hands the root file of every target in the workspace to ONE rustfmt
//! command. At ~800 files that command reached 32,774 characters from a worktree under
//! `Forge-Engine-lanes\`, past Windows' 32,767-character limit (os error 206), so
//! `just verify` stopped at its first step before anything was checked. Running
//! `cargo fmt -p ... -p ...` for a few packages at a time keeps every command short on any
//! path, and formats exactly what `--all` formats.

use std::path::Path;
use std::process::Command;

use crate::util::{Failure, XResult};

/// Packages per `cargo fmt` call. The whole workspace fit in 32,774 characters, so a
/// group of this size stays far below the limit however deep the checkout sits.
pub const GROUP: usize = 12;

pub fn run(root: &Path, check: bool) -> XResult<String> {
    let names = package_names(root)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut failed = Vec::new();
    for group in groups(&names) {
        let mut cmd = Command::new(&cargo);
        cmd.current_dir(root).arg("fmt");
        for name in group {
            cmd.arg("-p").arg(name);
        }
        if check {
            cmd.args(["--", "--check"]);
        }
        let status = cmd
            .status()
            .map_err(|e| Failure::one(format!("fmt: cannot run cargo fmt: {e}")))?;
        if !status.success() {
            failed.push(format!(
                "fmt: `cargo fmt` {} for {} (run `just fmt`)",
                if check {
                    "found unformatted code"
                } else {
                    "failed"
                },
                group.join(", ")
            ));
        }
    }
    Failure::many(failed)?;
    Ok(format!(
        "fmt: {} packages {}\n",
        names.len(),
        if check { "formatted" } else { "reformatted" }
    ))
}

/// The package groups, in name order, each at most [`GROUP`] long.
pub fn groups(names: &[String]) -> std::slice::Chunks<'_, String> {
    names.chunks(GROUP)
}

/// Every workspace package name, sorted, as cargo itself sees the workspace
/// (`cargo metadata --no-deps`). The member globs alone are not enough: a path dependency
/// of a member (crates/forge-reflect/macros) is a workspace package without matching one.
pub fn package_names(root: &Path) -> XResult<Vec<String>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|e| Failure::one(format!("fmt: cannot run cargo metadata: {e}")))?;
    if !out.status.success() {
        return Err(Failure::one(format!(
            "fmt: cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    names_from_metadata(&String::from_utf8_lossy(&out.stdout))
}

/// The package names in `cargo metadata` JSON, sorted.
pub fn names_from_metadata(json: &str) -> XResult<Vec<String>> {
    let meta: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| Failure::one(format!("fmt: cannot parse cargo metadata: {e}")))?;
    let mut names: Vec<String> = meta
        .get("packages")
        .and_then(|p| p.as_array())
        .ok_or_else(|| Failure::one("fmt: cargo metadata has no packages"))?
        .iter()
        .filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util;

    #[test]
    fn every_member_package_is_listed_and_excluded_dirs_are_not() {
        let names = package_names(&util::workspace_root()).expect("package names");
        // forge-reflect-macros is a workspace package only as a member's path dependency.
        for expected in [
            "xtask",
            "forge-gpu",
            "forge-editor",
            "forge-tests",
            "forge-reflect-macros",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "{expected} missing from {names:?}"
            );
        }
        // tools/docker is excluded in the root manifest (it is the Linux verify image, not a crate).
        assert!(!names.iter().any(|n| n.contains("docker")), "{names:?}");
    }

    #[test]
    fn groups_cover_every_package_once_and_stay_small() {
        let names = package_names(&util::workspace_root()).expect("package names");
        let grouped: Vec<&String> = groups(&names).flatten().collect();
        assert_eq!(grouped.len(), names.len());
        assert!(groups(&names).all(|g| !g.is_empty() && g.len() <= GROUP));
    }

    // Positive control (W2): the guard above is not vacuous — a single group holding the
    // whole workspace (what `cargo fmt --all` does) breaks the size bound.
    #[test]
    fn positive_control_one_group_for_the_whole_workspace_breaks_the_bound() {
        let names = package_names(&util::workspace_root()).expect("package names");
        assert!(
            names.len() > GROUP,
            "the workspace must span several groups ({} packages)",
            names.len()
        );
        let one: Vec<&[String]> = names.chunks(names.len()).collect();
        assert!(one.iter().any(|g| g.len() > GROUP));
    }
}
