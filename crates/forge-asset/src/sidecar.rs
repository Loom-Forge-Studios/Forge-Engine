//! The import sidecar: `<source>.meta.ron`, next to the source, committed with it.
//!
//! It is the asset's identity and import settings (what the user owns) plus the record of
//! the last import (a cache the server can always regenerate):
//!
//! ```ron
//! AssetMeta(
//!     id: "5f0c…",                       // minted once, travels with the file
//!     importer: "gltf",
//!     settings: { "srgb": "true" },      // import settings: diffable, mergeable
//!     import: Some(ImportRecord(
//!         importer: "gltf",
//!         importer_version: 1,
//!         source: "9ab1…",               // BLAKE3 of the source bytes
//!         source_size: Some(18234),       // its size in bytes
//!         settings: "03c4…",             // BLAKE3 of the settings it used
//!         deps: [ ("models/ship.bin", "77c2…") ],
//!         artefacts: [ ("", "…id", "scene", "Ship", "…blob") ],
//!     )),
//! )
//! ```
//!
//! Text, sorted, stable field order: a sidecar diff in a pull request reads as what changed
//! ("srgb: true -> false"), and two people changing different settings merge cleanly.

use std::collections::BTreeMap;

use bytes::Bytes;
use forge_store::{Blake3, StorePath};
use serde::{Deserialize, Serialize};

use crate::{AssetError, AssetId};

/// The suffix that marks a sidecar.
pub const SIDECAR_SUFFIX: &str = ".meta.ron";

/// Import settings: string keys to string values, sorted (diffable, mergeable).
pub type Settings = BTreeMap<String, String>;

/// One artefact of an import: `(label, id, kind, name, blob address)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtefactRecord(pub String, pub AssetId, pub String, pub String, pub String);

impl ArtefactRecord {
    /// The label (`""` for the main artefact).
    #[must_use]
    pub fn label(&self) -> &str {
        &self.0
    }
    /// The artefact's asset id.
    #[must_use]
    pub fn id(&self) -> AssetId {
        self.1
    }
    /// Its kind.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.2
    }
    /// Its display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.3
    }
    /// Its blob address.
    pub fn blob(&self) -> Result<Blake3, AssetError> {
        self.4.parse().map_err(|_| AssetError::Sidecar {
            path: String::new(),
            why: format!("bad blob address {:?}", self.4),
        })
    }
}

/// The record of the last import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRecord {
    /// The importer that made it (a sidecar edited to name another one is stale).
    pub importer: String,
    /// The importer's version at the time.
    pub importer_version: u32,
    /// BLAKE3 of the source bytes.
    pub source: String,
    /// The source's size in bytes (lets hot reload skip files that cannot be a renamed
    /// source without reading them). Absent in sidecars written before it was recorded.
    #[serde(default)]
    pub source_size: Option<u64>,
    /// BLAKE3 of the settings it was made with (see [`settings_hash`]).
    pub settings: String,
    /// Every other file the import read, with its BLAKE3.
    pub deps: Vec<(String, String)>,
    /// What it produced.
    pub artefacts: Vec<ArtefactRecord>,
}

/// The address of a settings map (sorted `key=value` lines), recorded with each import so
/// an edited setting makes the record stale.
#[must_use]
pub fn settings_hash(s: &Settings) -> String {
    let mut h = blake3::Hasher::new();
    for (k, v) in s {
        h.update(&(k.len() as u64).to_le_bytes());
        h.update(k.as_bytes());
        h.update(&(v.len() as u64).to_le_bytes());
        h.update(v.as_bytes());
    }
    Blake3(*h.finalize().as_bytes()).to_hex()
}

/// The sidecar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "AssetMeta")]
pub struct Sidecar {
    /// The asset's stable id.
    pub id: AssetId,
    /// The importer (registry key).
    pub importer: String,
    /// Import settings.
    #[serde(default)]
    pub settings: Settings,
    /// The last import, if any.
    #[serde(default)]
    pub import: Option<ImportRecord>,
}

impl Sidecar {
    /// The sidecar path for `source`.
    pub fn path_for(source: &StorePath) -> Result<StorePath, AssetError> {
        Ok(StorePath::new(&format!("{source}{SIDECAR_SUFFIX}"))?)
    }

    /// The source a sidecar path belongs to (`None`: not a sidecar path).
    #[must_use]
    pub fn source_of(sidecar: &StorePath) -> Option<StorePath> {
        sidecar
            .as_str()
            .strip_suffix(SIDECAR_SUFFIX)
            .and_then(|s| StorePath::new(s).ok())
    }

    /// Canonical text.
    pub fn encode(&self) -> Result<Bytes, AssetError> {
        let cfg = ron::ser::PrettyConfig::new()
            .new_line("\n".to_string())
            .struct_names(true);
        ron::ser::to_string_pretty(self, cfg)
            .map(|s| Bytes::from(s + "\n"))
            .map_err(|e| AssetError::Sidecar {
                path: String::new(),
                why: e.to_string(),
            })
    }

    /// Parse sidecar text.
    pub fn decode(path: &StorePath, b: &[u8]) -> Result<Self, AssetError> {
        let bad = |why: String| AssetError::Sidecar {
            path: path.to_string(),
            why,
        };
        let text = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
        ron::from_str(text).map_err(|e| bad(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_text_round_trips() {
        let s = Sidecar {
            id: AssetId(42),
            importer: "gltf".into(),
            settings: [("srgb".to_string(), "true".to_string())].into(),
            import: Some(ImportRecord {
                importer: "gltf".into(),
                importer_version: 1,
                source: Blake3::of(b"a").to_hex(),
                source_size: Some(1),
                settings: settings_hash(&Settings::new()),
                deps: vec![("m/a.bin".into(), Blake3::of(b"b").to_hex())],
                artefacts: vec![ArtefactRecord(
                    String::new(),
                    AssetId(42),
                    "scene".into(),
                    "S".into(),
                    Blake3::of(b"c").to_hex(),
                )],
            }),
        };
        let p = StorePath::new("m/a.gltf.meta.ron").expect("p");
        let b = s.encode().expect("encodes");
        assert!(
            std::str::from_utf8(&b)
                .expect("utf8")
                .starts_with("AssetMeta(")
        );
        assert_eq!(Sidecar::decode(&p, &b).expect("decodes"), s);
        assert_eq!(
            Sidecar::source_of(&p).map(|s| s.to_string()).as_deref(),
            Some("m/a.gltf")
        );
        assert!(Sidecar::decode(&p, b"garbage(").is_err());
    }
}
