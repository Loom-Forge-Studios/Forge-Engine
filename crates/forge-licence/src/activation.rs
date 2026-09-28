//! Where the signed entitlement is read from, and loading a [`Gate`] from it (ADR 0063). The
//! two location strings — the `FORGE_ENTITLEMENT` override and the `entitlement.json`
//! filename — are **licensing identifiers**, so they go through [`obf!`](crate::obf) and are
//! not plain text in a shipped premium binary (`premium-protections` proves their absence).
//!
//! This lives in `forge-licence`, not in each premium binary, so the premium editor and the
//! premium headless CLI resolve and load the entitlement by exactly the same rule (no drift),
//! and the droplet server can reuse it.

use std::path::{Path, PathBuf};

use crate::gate::{Gate, GateFileError};

/// The entitlement file path: `$FORGE_ENTITLEMENT` if set (activation and the tests point it
/// at a file), else `entitlement.json` in `config_dir` (the per-user config directory the
/// caller supplies). `None` when neither is available (no override and no config dir).
#[must_use]
pub fn entitlement_path(config_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(crate::obf!("FORGE_ENTITLEMENT")) {
        return Some(PathBuf::from(p));
    }
    config_dir.map(|d| d.join(crate::obf!("entitlement.json")))
}

/// Load and verify the entitlement offline (see [`entitlement_path`]). Returns a base
/// [`Gate`] plus, when a present file did not verify, the [`GateFileError`] the caller turns
/// into a localised, non-fatal message. A missing file, or no path at all, is a base gate
/// with no error (an un-activated machine is base, not an error — E-57: never locked out).
#[must_use]
pub fn load_gate(config_dir: Option<&Path>) -> (Gate, Option<GateFileError>) {
    match entitlement_path(config_dir) {
        Some(path) => match Gate::from_file(&path) {
            Ok(gate) => (gate, None),
            // The file was present but forged/unreadable: degrade to base, hand the reason back.
            Err(e) => (Gate::base(), Some(e)),
        },
        None => (Gate::base(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_prefers_the_env_override_then_the_config_dir() {
        // Note: this reads the process environment; it does not set it (parallel-test safe).
        let cfg = PathBuf::from("/cfg");
        // With no override in this process, the config dir decides.
        if std::env::var_os(crate::obf!("FORGE_ENTITLEMENT")).is_none() {
            assert_eq!(
                entitlement_path(Some(&cfg)),
                Some(cfg.join("entitlement.json"))
            );
            assert_eq!(entitlement_path(None), None);
        }
    }

    #[test]
    fn a_missing_entitlement_is_a_base_gate_without_an_error() {
        let (gate, err) = load_gate(Some(Path::new("/does/not/exist")));
        assert!(err.is_none(), "a missing file is not an error");
        assert!(!gate.premium_enabled(0, "0.1.0"));
    }
}
