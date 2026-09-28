//! `AssetError` — the `ASSET-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};
use forge_store::StoreError;

/// Every way an asset operation can fail. Converts into `forge_core::Error` with `?`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AssetError {
    /// `ASSET-0001`: a reference is not a portable project path: backslash separators, an
    /// absolute path, a scheme the VFS does not know, or `..` above the project root.
    BadReference {
        /// The reference as written.
        reference: String,
        /// The file that contains it.
        from: String,
        /// Why.
        why: String,
    },
    /// `ASSET-0002`: a reference resolves only case-insensitively (M0-17). It would load on
    /// Windows and fail on Linux, so it fails everywhere.
    CaseMismatch {
        /// The reference as written.
        reference: String,
        /// The file that contains it.
        from: String,
        /// The name as it is in the store.
        on_disk: String,
    },
    /// `ASSET-0003`: a reference names nothing.
    MissingReference {
        /// The reference as written.
        reference: String,
        /// The file that contains it.
        from: String,
    },
    /// `ASSET-0004`: no registered importer handles this file.
    NoImporter(String),
    /// `ASSET-0005`: an importer rejected its input (malformed or unsupported file).
    Import {
        /// The source file.
        path: String,
        /// The importer.
        importer: String,
        /// Why.
        why: String,
    },
    /// `ASSET-0006`: no asset has this id or path.
    UnknownAsset(String),
    /// `ASSET-0007`: a handle asked for a different type than the asset's kind decodes to.
    WrongType {
        /// The asset.
        id: String,
        /// Its kind.
        kind: String,
        /// The type the handle wanted.
        wanted: &'static str,
    },
    /// `ASSET-0008`: no `AssetType` is registered for this kind.
    UnknownKind(String),
    /// `ASSET-0009`: an artefact's bytes do not decode as its kind.
    Decode {
        /// The asset.
        id: String,
        /// Why.
        why: String,
    },
    /// `ASSET-0010`: the project store failed (its own code follows).
    Store(StoreError),
    /// `ASSET-0011`: an import sidecar (`*.meta.ron`) does not parse.
    Sidecar {
        /// The sidecar.
        path: String,
        /// Why.
        why: String,
    },
    /// `ASSET-0012`: a rename or import target already exists.
    AlreadyExists(String),
    /// `ASSET-0013`: a load did not finish in the time the caller allowed.
    Timeout(String),
    /// `ASSET-0014`: a generated asset's generator is unknown or failed.
    Generator {
        /// The generator.
        name: String,
        /// Why.
        why: String,
    },
    /// `ASSET-0015`: an exporter failed or none handles the asset.
    Export {
        /// The asset.
        id: String,
        /// Why.
        why: String,
    },
}

impl AssetError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::BadReference { .. } => error_code!("ASSET-0001"),
            Self::CaseMismatch { .. } => error_code!("ASSET-0002"),
            Self::MissingReference { .. } => error_code!("ASSET-0003"),
            Self::NoImporter(_) => error_code!("ASSET-0004"),
            Self::Import { .. } => error_code!("ASSET-0005"),
            Self::UnknownAsset(_) => error_code!("ASSET-0006"),
            Self::WrongType { .. } => error_code!("ASSET-0007"),
            Self::UnknownKind(_) => error_code!("ASSET-0008"),
            Self::Decode { .. } => error_code!("ASSET-0009"),
            Self::Store(_) => error_code!("ASSET-0010"),
            Self::Sidecar { .. } => error_code!("ASSET-0011"),
            Self::AlreadyExists(_) => error_code!("ASSET-0012"),
            Self::Timeout(_) => error_code!("ASSET-0013"),
            Self::Generator { .. } => error_code!("ASSET-0014"),
            Self::Export { .. } => error_code!("ASSET-0015"),
        }
    }

    pub(crate) fn import(path: impl fmt::Display, importer: &str, why: impl fmt::Display) -> Self {
        Self::Import {
            path: path.to_string(),
            importer: importer.to_string(),
            why: why.to_string(),
        }
    }

    pub(crate) fn decode(id: impl fmt::Display, why: impl fmt::Display) -> Self {
        Self::Decode {
            id: id.to_string(),
            why: why.to_string(),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<AssetError> {
        let s = || "x".to_string();
        vec![
            Self::BadReference {
                reference: s(),
                from: s(),
                why: s(),
            },
            Self::CaseMismatch {
                reference: s(),
                from: s(),
                on_disk: s(),
            },
            Self::MissingReference {
                reference: s(),
                from: s(),
            },
            Self::NoImporter(s()),
            Self::Import {
                path: s(),
                importer: s(),
                why: s(),
            },
            Self::UnknownAsset(s()),
            Self::WrongType {
                id: s(),
                kind: s(),
                wanted: "T",
            },
            Self::UnknownKind(s()),
            Self::Decode { id: s(), why: s() },
            Self::Store(StoreError::NotFound(s())),
            Self::Sidecar {
                path: s(),
                why: s(),
            },
            Self::AlreadyExists(s()),
            Self::Timeout(s()),
            Self::Generator {
                name: s(),
                why: s(),
            },
            Self::Export { id: s(), why: s() },
        ]
    }
}

impl From<StoreError> for AssetError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::BadReference {
                reference,
                from,
                why,
            } => write!(f, "{reference:?} in {from} is not a project path: {why}"),
            Self::CaseMismatch {
                reference,
                from,
                on_disk,
            } => write!(
                f,
                "{reference:?} in {from} matches {on_disk:?} only by ignoring letter case (it would load on Windows and fail on Linux)"
            ),
            Self::MissingReference { reference, from } => {
                write!(f, "{reference:?} in {from} names no file")
            }
            Self::NoImporter(p) => write!(f, "no importer handles {p}"),
            Self::Import {
                path,
                importer,
                why,
            } => write!(f, "importer {importer:?} could not import {path}: {why}"),
            Self::UnknownAsset(what) => write!(f, "no asset {what}"),
            Self::WrongType { id, kind, wanted } => {
                write!(f, "asset {id} is a {kind:?}, not a {wanted}")
            }
            Self::UnknownKind(k) => write!(f, "no asset type is registered for kind {k:?}"),
            Self::Decode { id, why } => write!(f, "asset {id} does not decode: {why}"),
            Self::Store(e) => write!(f, "the project store failed: {e}"),
            Self::Sidecar { path, why } => write!(f, "import sidecar {path} is invalid: {why}"),
            Self::AlreadyExists(p) => write!(f, "{p} already exists"),
            Self::Timeout(id) => write!(f, "asset {id} did not load in time"),
            Self::Generator { name, why } => write!(f, "generator {name:?}: {why}"),
            Self::Export { id, why } => write!(f, "could not export asset {id}: {why}"),
        }
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl CodedError for AssetError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
