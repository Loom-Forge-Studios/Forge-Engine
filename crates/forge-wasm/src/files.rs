//! **A plugin directory's files, read once** (WP-36, ADR 0045 Amendment 3).
//!
//! A host that decides whether to run a plugin by what its files hold (the editor's project
//! trust keys each project plugin by a digest of its manifest and code) must compile **the
//! bytes it decided about**. Reading the files once to vet them and again to compile them
//! leaves a window in which a writer (a `git pull` in a terminal) swaps the code: the
//! second read compiles bytes nobody vetted. [`PluginFiles`] is the one read: the host vets
//! it and compiles from it ([`crate::WasmHost::load_files`]), and a plugin loaded from it
//! remembers what it was compiled from ([`Baseline`]), so the hot-reload watcher starts from
//! those bytes too instead of reading the files a third time.
//!
//! The stamps (length and modification time) are taken **before** the bytes are read: a
//! write after the stamp moves the stamp, so the watcher's next poll reads the files again.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use forge_plugin::Manifest;

use crate::WasmError;
use crate::host::{code_file, read_file};

/// A file's length and modification time: what a poll compares before reading.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

pub(crate) fn stamp(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    Some(Stamp {
        len: m.len(),
        modified: m.modified().ok(),
    })
}

/// The watcher's content hash (not a security digest: it only tells a rewrite of the same
/// bytes from a change).
pub(crate) fn hash(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// A plugin directory's manifest and code as one read found them (see the module docs).
#[derive(Clone)]
pub struct PluginFiles {
    manifest: Vec<u8>,
    /// The code file and its bytes; `None`: the directory has no `plugin.wasm` / `plugin.wat`.
    code: Option<(PathBuf, Vec<u8>)>,
    manifest_stamp: Option<Stamp>,
    code_stamp: Option<Stamp>,
}

impl std::fmt::Debug for PluginFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginFiles")
            .field("manifest", &self.manifest.len())
            .field(
                "code",
                &self
                    .code
                    .as_ref()
                    .map(|(p, b)| (p.display().to_string(), b.len())),
            )
            .finish_non_exhaustive()
    }
}

impl PluginFiles {
    /// Read `dir`'s `plugin.ron` and its code file (`plugin.wasm`, else `plugin.wat`), each
    /// once, stamps first. `Err`: the manifest (or an existing code file) could not be read.
    pub fn read(dir: &Path) -> Result<Self, WasmError> {
        let manifest_path = dir.join("plugin.ron");
        let manifest_stamp = stamp(&manifest_path);
        let manifest = read_file(&manifest_path)?;
        let (code, code_stamp) = match code_file(dir) {
            Some(p) => {
                let s = stamp(&p);
                let bytes = read_file(&p)?;
                (Some((p, bytes)), s)
            }
            None => (None, None),
        };
        Ok(Self {
            manifest,
            code,
            manifest_stamp,
            code_stamp,
        })
    }

    /// Files held in memory (the watcher's view of a changed code file under the manifest in
    /// effect; tests). No stamps: nothing on disk is claimed.
    #[must_use]
    pub fn from_parts(manifest: Vec<u8>, code: Option<(PathBuf, Vec<u8>)>) -> Self {
        Self {
            manifest,
            code,
            manifest_stamp: None,
            code_stamp: None,
        }
    }

    /// The manifest's bytes.
    #[must_use]
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }

    /// The code file's name (`plugin.wasm` / `plugin.wat`) and its bytes.
    #[must_use]
    pub fn code(&self) -> Option<(&str, &[u8])> {
        self.code.as_ref().map(|(p, b)| {
            (
                p.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                b.as_slice(),
            )
        })
    }

    /// The manifest, parsed from these bytes.
    pub fn manifest(&self) -> Result<Manifest, WasmError> {
        let text = std::str::from_utf8(&self.manifest).map_err(|e| WasmError::Io {
            path: "plugin.ron".into(),
            why: e.to_string(),
        })?;
        Ok(Manifest::parse(text)?)
    }

    /// What a plugin compiled from these files remembers (see [`Baseline`]).
    pub(crate) fn baseline(&self) -> Baseline {
        Baseline {
            manifest: self.manifest.clone(),
            manifest_stamp: self.manifest_stamp,
            code_path: self.code.as_ref().map(|(p, _)| p.clone()),
            code_stamp: self.code_stamp,
            code_hash: self.code.as_ref().map_or(0, |(_, b)| hash(b)),
        }
    }
}

/// What a plugin was compiled from: its manifest's bytes, the stamps taken before they were
/// read, and its code's hash — the hot-reload watcher's starting point (no code bytes are
/// kept).
#[derive(Clone, Debug)]
pub(crate) struct Baseline {
    pub(crate) manifest: Vec<u8>,
    pub(crate) manifest_stamp: Option<Stamp>,
    pub(crate) code_path: Option<PathBuf>,
    pub(crate) code_stamp: Option<Stamp>,
    pub(crate) code_hash: u64,
}
