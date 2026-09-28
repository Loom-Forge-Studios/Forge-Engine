//! Small shared helpers: locating the workspace root, reading files, and the error type.

use std::fmt;
use std::path::{Path, PathBuf};

/// A failed xtask check. Every message names the file and the rule it enforces, so a red
/// `just verify` says what to fix without reading xtask source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub messages: Vec<String>,
}

impl Failure {
    pub fn one(msg: impl Into<String>) -> Self {
        Self {
            messages: vec![msg.into()],
        }
    }

    pub fn many(messages: Vec<String>) -> Result<(), Self> {
        if messages.is_empty() {
            Ok(())
        } else {
            Err(Self { messages })
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for m in &self.messages {
            writeln!(f, "error: {m}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

pub type XResult<T> = Result<T, Failure>;

/// The workspace root: the directory above `xtask/`. Taken from the compile-time manifest
/// dir so it works from any current directory.
pub fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest.to_path_buf())
}

pub fn read(path: &Path) -> XResult<String> {
    std::fs::read_to_string(path)
        .map(|s| s.replace("\r\n", "\n"))
        .map_err(|e| Failure::one(format!("cannot read {}: {e}", path.display())))
}

/// Reasons that are not reasons (W9): "nobody got to it" is never a legitimate state.
pub fn is_banned_reason(reason: &str) -> bool {
    let r = reason.trim().to_ascii_lowercase();
    const BANNED: [&str; 10] = [
        "",
        "todo",
        "tbd",
        "wip",
        "later",
        "not done",
        "not yet",
        "nobody got to it",
        "no time",
        "n/a",
    ];
    BANNED.contains(&r.as_str()) || r.contains("nobody got to it") || r.len() < 12
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banned_reasons_are_rejected_and_real_ones_pass() {
        assert!(is_banned_reason("TODO"));
        assert!(is_banned_reason("  nobody got to it  "));
        assert!(is_banned_reason("later"));
        assert!(!is_banned_reason(
            "forge-num does not exist until WP-01 (M0-2)"
        ));
    }

    #[test]
    fn workspace_root_contains_the_justfile() {
        assert!(workspace_root().join("justfile").is_file());
    }
}
