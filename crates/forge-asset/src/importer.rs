//! The asset extension points (Ch.32.2 seed list): `Importer`, `Exporter`, `AssetType`.
//!
//! Importers are plugins like any other (I16): the first-party glTF, image and KTX2 importers
//! register through the ordinary loader ([`crate::FirstPartyAssets`]), and a third-party
//! plugin can add an importer (`fbx`), replace one, or chain one (post-process every imported
//! mesh) with the same four operations.
//!
//! An importer is a pure function of its inputs: the source bytes, its settings, and the
//! files it reads through [`ImportCx::dependency`] (resolved exactly, M0-17, and recorded so
//! a change to any of them reimports). Its outputs are artefacts — kind-tagged byte blobs
//! stored content-addressed in the project's blob store. Because an importer never sees a
//! filesystem, the same import gives the same artefact addresses on every backend.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use forge_plugin::ExtensionPoint;
use forge_store::{Blake3, StorePath};

use crate::sidecar::Settings;
use crate::types::AssetValue;
use crate::{AssetError, AssetId, Vfs};

/// One output of an import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artefact {
    /// Stable label within the import (`""` for the main artefact, `mesh:Hull`, ...).
    pub label: String,
    /// Its kind (an `AssetType` key).
    pub kind: String,
    /// Display name.
    pub name: String,
    /// The bytes.
    pub bytes: Bytes,
}

/// What an importer sees.
pub struct ImportCx<'a> {
    pub(crate) id: AssetId,
    pub(crate) importer: &'a str,
    pub(crate) path: &'a StorePath,
    pub(crate) source: Bytes,
    pub(crate) settings: &'a Settings,
    pub(crate) vfs: &'a Vfs,
    pub(crate) deps: BTreeMap<StorePath, Blake3>,
    pub(crate) artefacts: Vec<Artefact>,
}

impl ImportCx<'_> {
    /// The asset's id (sub-assets are `id().child(label)`).
    #[must_use]
    pub fn id(&self) -> AssetId {
        self.id
    }

    /// The source's project path.
    #[must_use]
    pub fn path(&self) -> &StorePath {
        self.path
    }

    /// The source bytes.
    #[must_use]
    pub fn source(&self) -> &Bytes {
        &self.source
    }

    /// An import setting.
    #[must_use]
    pub fn setting(&self, key: &str) -> Option<&str> {
        self.settings.get(key).map(String::as_str)
    }

    /// Every import setting (a sandboxed importer gets them all with the source: WP-21).
    #[must_use]
    pub fn settings(&self) -> &Settings {
        self.settings
    }

    /// A boolean setting (`"true"` / `"false"`), with a default.
    #[must_use]
    pub fn flag(&self, key: &str, default: bool) -> bool {
        match self.setting(key) {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        }
    }

    /// Read another file the source references (relative to the source), resolved exactly
    /// (M0-17), and record it as a dependency: when it changes, this asset reimports.
    pub fn dependency(&mut self, reference: &str) -> Result<Bytes, AssetError> {
        let p = self.vfs.resolve(self.path, reference)?;
        let b = self.vfs.read(&p)?;
        self.deps.insert(p, Blake3::of(&b));
        Ok(b)
    }

    /// Emit an artefact; returns its asset id. `label` must be unique within the import;
    /// exactly one artefact has the empty label (the main one).
    pub fn emit(&mut self, label: &str, kind: &str, name: &str, bytes: Bytes) -> AssetId {
        self.artefacts.push(Artefact {
            label: label.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            bytes,
        });
        self.id.child(label)
    }

    /// Emit an encoded [`AssetValue`]. An encoding failure fails the import (naming the
    /// source, the importer and the artefact) instead of storing an empty artefact.
    pub fn emit_value<T: AssetValue>(
        &mut self,
        label: &str,
        name: &str,
        v: &T,
    ) -> Result<AssetId, AssetError> {
        let bytes = v
            .encode()
            .map_err(|e| self.fail(format!("encoding {} `{name}`: {e}", T::KIND)))?;
        Ok(self.emit(label, T::KIND, name, bytes))
    }

    /// An import error for this source.
    pub fn fail(&self, why: impl std::fmt::Display) -> AssetError {
        AssetError::import(self.path, self.importer, why)
    }

    /// The artefacts emitted so far. A middleware importer (registered with `chain`) runs
    /// the importer it wraps, then post-processes these — renames, re-encodes, adds.
    pub fn artefacts_mut(&mut self) -> &mut Vec<Artefact> {
        &mut self.artefacts
    }
}

/// Run `importer` on `path` without storing anything: the artefacts it would produce (the
/// editor's import preview; conformance tests). A panicking importer is `ASSET-0005`.
pub fn dry_import(
    importer: &dyn Importer,
    vfs: &Vfs,
    path: &StorePath,
    settings: &Settings,
) -> Result<Vec<Artefact>, AssetError> {
    let source = vfs.read(path)?;
    let id = AssetId::mint(path, &Blake3::of(&source));
    let mut cx = ImportCx {
        id,
        importer: "(dry run)",
        path,
        source,
        settings,
        vfs,
        deps: BTreeMap::new(),
        artefacts: Vec::new(),
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| importer.import(&mut cx))) {
        Ok(r) => r?,
        Err(_) => {
            return Err(AssetError::import(
                path,
                "(dry run)",
                "the importer panicked",
            ));
        }
    }
    Ok(cx.artefacts)
}

/// An importer.
pub trait Importer: Send + Sync {
    /// Bumped whenever its output for the same input changes: every asset it imported then
    /// reimports on open.
    fn version(&self) -> u32;
    /// The lower-case file extensions it claims (`["gltf", "glb"]`).
    fn extensions(&self) -> &[&str];
    /// Import `cx.source()` into artefacts.
    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError>;
}

/// The `Importer` point (`forge.asset.importer`); the key is the importer id.
pub struct ImporterPoint;

impl ExtensionPoint for ImporterPoint {
    type Item = Arc<dyn Importer>;
    const ID: &'static str = "forge.asset.importer";
    const NAME: &'static str = "Importer";
}

/// Decodes an artefact to its runtime value.
pub type DecodeFn = Arc<dyn Fn(&Bytes) -> Result<Arc<dyn Any + Send + Sync>, String> + Send + Sync>;

/// A registered asset type: how to decode one kind of artefact.
#[derive(Clone)]
pub struct AssetTypeItem {
    /// The Rust type's name (for errors).
    pub type_name: &'static str,
    /// The Rust type a decode yields (typed handles check it).
    pub type_id: TypeId,
    /// The decoder.
    pub decode: DecodeFn,
}

impl AssetTypeItem {
    /// The item for an [`AssetValue`] type.
    #[must_use]
    pub fn of<T: AssetValue>() -> Self {
        Self {
            type_name: std::any::type_name::<T>(),
            type_id: TypeId::of::<T>(),
            decode: Arc::new(|b: &Bytes| {
                T::decode(b).map(|v| Arc::new(v) as Arc<dyn Any + Send + Sync>)
            }),
        }
    }
}

/// The `AssetType` point (`forge.asset.type`); the key is the kind.
pub struct AssetTypePoint;

impl ExtensionPoint for AssetTypePoint {
    type Item = AssetTypeItem;
    const ID: &'static str = "forge.asset.type";
    const NAME: &'static str = "AssetType";
}

/// One artefact as an exporter sees it.
#[derive(Clone, Debug)]
pub struct ExportArtefact {
    /// Its kind.
    pub kind: String,
    /// Its display name.
    pub name: String,
    /// Its bytes.
    pub bytes: Bytes,
}

/// What an exporter sees: artefacts by id.
pub trait ExportSource {
    /// The artefact of `id`.
    fn artefact(&self, id: AssetId) -> Result<ExportArtefact, AssetError>;
}

/// An exporter: artefacts back to an interchange format.
pub trait Exporter: Send + Sync {
    /// The kinds it can export as the root asset.
    fn kinds(&self) -> &[&str];
    /// Export `root` (and whatever it references) as files: `(relative name, bytes)`, the
    /// first being the main file.
    fn export(
        &self,
        src: &dyn ExportSource,
        root: AssetId,
        name: &str,
    ) -> Result<Vec<(String, Bytes)>, AssetError>;
}

/// The `Exporter` point (`forge.asset.exporter`); the key is the exporter id.
pub struct ExporterPoint;

impl ExtensionPoint for ExporterPoint {
    type Item = Arc<dyn Exporter>;
    const ID: &'static str = "forge.asset.exporter";
    const NAME: &'static str = "Exporter";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value whose encoding fails (a plugin's type that cannot represent some state).
    struct Unencodable;

    impl AssetValue for Unencodable {
        const KIND: &'static str = "unencodable";
        fn decode(_: &Bytes) -> Result<Self, String> {
            Ok(Self)
        }
        fn encode(&self) -> Result<Bytes, String> {
            Err("no representation for this state".into())
        }
    }

    struct EmitsIt;

    impl Importer for EmitsIt {
        fn version(&self) -> u32 {
            1
        }
        fn extensions(&self) -> &[&str] {
            &["x"]
        }
        fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
            cx.emit("side", "blob", "side", Bytes::from_static(b"ok"));
            cx.emit_value("", "main", &Unencodable)?;
            Ok(())
        }
    }

    #[test]
    fn an_encoding_failure_fails_the_import_instead_of_storing_empty_bytes() {
        let mut store = forge_store::MemoryStore::new("ada");
        let path = StorePath::new("a.x").expect("path");
        forge_store::ProjectStore::write(&mut store, &path, Bytes::from_static(b"src"))
            .expect("write");
        let vfs = Vfs::new(Box::new(store));
        let e = dry_import(&EmitsIt, &vfs, &path, &Settings::new())
            .expect_err("an unencodable value must fail the import");
        let text = e.to_string();
        assert!(
            text.contains("no representation for this state") && text.contains("a.x"),
            "the error names the source and the cause: {text}"
        );
    }
}
