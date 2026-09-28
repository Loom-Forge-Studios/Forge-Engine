//! `forge-asset` — the asset system (Ch.8): identity, import, content addressing, loading,
//! hot reload, thumbnails.
//!
//! * **Stable identity.** An [`AssetId`] is minted once and lives in the source's sidecar
//!   (`<source>.meta.ron`); renaming or moving the file keeps it, so references survive.
//!   Sub-assets (a glTF's meshes) and generated assets (a `SeedPath`, no file) derive theirs.
//! * **Content addressed.** Importers produce artefacts stored by BLAKE3 in the project's
//!   blob store (forge-store, Ch.33.2). Unchanged inputs never re-run an importer; unchanged
//!   artefacts never reload.
//! * **Storage-agnostic.** All I/O goes through the [`Vfs`] over the `ProjectStore` trait
//!   (I17) — no `std::fs` in this crate. References inside assets resolve **exactly**
//!   (M0-17): a wrong-case glTF `uri` fails on Windows too.
//! * **Plugins.** Importers, exporters and asset types are extension points
//!   ([`ImporterPoint`], [`ExporterPoint`], [`AssetTypePoint`]). The kernel's asset types load
//!   through the ordinary loader as [`FirstPartyAssets`]; every importer and exporter is a
//!   plugin — the first-party glTF, image and KTX2 ones are `plugins/forge-importers` — using
//!   only this crate's public API (I16).
//! * **Typed async handles.** [`AssetServer::load`] returns a [`Handle<T>`] at once; worker
//!   threads decode; hot reload swaps values under live handles.
//! * **Commands.** Import, remove and rename are bus commands ([`commands`], I7); the server
//!   follows the project with [`AssetServer::sync`].
//!
//! ```
//! use forge_asset::{AssetServer, MeshAsset, SceneAsset, fixture};
//! use forge_store::{MemoryStore, ProjectStore, StorePath};
//! use std::time::Duration;
//!
//! let mut store = MemoryStore::new("ada");
//! for (name, bytes) in fixture::scene(&fixture::SceneSpec::default()) {
//!     store.write(&StorePath::new(&format!("models/{name}"))?, bytes)?;
//! }
//! // The importers are a plugin (`forge.importers`), loaded with the kernel's asset types.
//! let (mut assets, _) = AssetServer::open(Box::new(store), &forge_importers::asset_extensions()?)?;
//! let mut events = Vec::new();
//! let id = assets.import(&StorePath::new("models/scene.gltf")?, Default::default(), &mut events)?;
//! let scene = assets.load::<SceneAsset>(id).wait(Duration::from_secs(10))?;
//! let first = scene.nodes[0].mesh.expect("a mesh");
//! let mesh = assets.load::<MeshAsset>(first).wait(Duration::from_secs(10))?;
//! assert_eq!(mesh.primitives[0].vertex_count, 25);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod bin;
pub mod commands;
mod error;
pub mod fixture;
mod handle;
mod id;
mod importer;
mod ktx2w;
mod loader;
mod plugin;
mod server;
mod sidecar;
mod texture;
mod thumb;
pub mod types;
mod vfs;

pub use error::AssetError;
pub use handle::{Handle, LoadState, Ready};
pub use id::AssetId;
pub use importer::{
    Artefact, AssetTypeItem, AssetTypePoint, DecodeFn, ExportArtefact, ExportSource, Exporter,
    ExporterPoint, ImportCx, Importer, ImporterPoint, dry_import,
};
pub use plugin::FirstPartyAssets;
pub use server::{AssetEvent, AssetInfo, AssetServer, AssetSource, GeneratorFn};
pub use sidecar::{ArtefactRecord, ImportRecord, SIDECAR_SUFFIX, Settings, Sidecar, settings_hash};
pub use texture::texture_from_rgba8;
pub use thumb::{Thumbnail, render_thumbnail};
pub use types::{
    AssetValue, MaterialAsset, MeshAsset, Primitive, SceneAsset, SceneNode, TextureAsset,
};
pub use vfs::{SharedStore, Vfs};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_asset_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in AssetError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-asset |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }

    #[test]
    fn the_first_party_manifest_parses_and_loads() {
        assert!(FirstPartyAssets::new().is_ok());
        let x = AssetServer::default_extensions().expect("loads");
        assert_eq!(
            x.registry::<ImporterPoint>().map(|r| r.len()),
            Some(0),
            "importers are plugins"
        );
        assert_eq!(x.registry::<AssetTypePoint>().map(|r| r.len()), Some(4));
    }
}
