//! Per-user configuration files (layouts, keymap, palette history) — user config, never
//! project state (Ch.21 §21.17, §21.18), so they are written directly, not through the bus.
//! These writers are the reasoned entries of the I7 allow-list
//! (`tests/liveness/i7_allow.txt`).
//!
//! Writes are **crash-safe**: the new content goes to a sibling temporary file which is
//! then renamed over the target (an atomic replace on Windows and POSIX), so a crash or a
//! power cut leaves either the old file or the new one, never a torn one.

use std::path::{Path, PathBuf};

use crate::EditorError;

/// The per-user configuration directory: `FORGE_CONFIG_DIR` if set (portable installs,
/// tests), else `%APPDATA%\Forge` on Windows, else `$XDG_CONFIG_HOME/forge`, else
/// `$HOME/.config/forge`.
pub fn user_config_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = env("FORGE_CONFIG_DIR") {
        return Some(d);
    }
    if cfg!(windows)
        && let Some(a) = env("APPDATA")
    {
        return Some(a.join("Forge"));
    }
    if let Some(x) = env("XDG_CONFIG_HOME") {
        return Some(x.join("forge"));
    }
    env("HOME").map(|h| h.join(".config").join("forge"))
}

fn io(path: &Path, e: std::io::Error) -> EditorError {
    EditorError::Io(format!("{}: {e}", path.display()))
}

/// The temporary sibling a write goes through.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Write `bytes` to `path` crash-safely (write the temporary sibling, then rename it over).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), EditorError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    }
    let tmp = temp_path(path);
    std::fs::write(&tmp, bytes).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io(path, e))
}

/// Read a config file; `Ok(None)` if it does not exist. A leftover temporary file from an
/// interrupted write is ignored: the last complete file is what loads.
pub fn read_optional(path: &Path) -> Result<Option<String>, EditorError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path, e)),
    }
}
