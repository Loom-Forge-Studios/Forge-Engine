//! `forge-importers` — the first-party asset importers and exporter, as a plugin (M2-14).
//!
//! `forge.importers` provides `Importer("gltf")`, `Importer("image")`, `Importer("ktx2")` and
//! `Exporter("gltf")` on `forge-asset`'s extension points. It is an ordinary source plugin:
//! it reaches the asset system only through `forge-asset`'s public API (`ImportCx`,
//! `Importer`, the asset types, `texture_from_rgba8`) and installs through the ordinary
//! loader, so anything it does a third-party importer can do too (I16,
//! `tests/plugin/test_no_privileged_plugin.rs`). A project can replace `gltf`, chain it (post-
//! process every imported mesh), or remove `image` in favour of its own decoder.
//!
//! ```
//! let x = forge_importers::asset_extensions()?;
//! let importers = x.registry::<forge_asset::ImporterPoint>().expect("defined");
//! assert_eq!(importers.provenance("gltf").map(|p| p.owner.as_str()), Some("forge.importers"));
//! # Ok::<(), forge_asset::AssetError>(())
//! ```

#![forbid(unsafe_code)]

mod gltf_export;
mod gltf_import;
mod image_import;

use std::sync::Arc;

use forge_asset::{
    AssetError, AssetServer, ExporterPoint, FirstPartyAssets, Importer, ImporterPoint,
};
use forge_plugin::{
    Extensions, Grants, InstallCx, Manifest, Order, PluginError, SourcePlugin, loader,
};

pub use gltf_export::GltfExporter;
pub use gltf_import::GltfImporter;
pub use image_import::{ImageImporter, Ktx2Importer, decode_image};

const MANIFEST: &str = r#"Plugin(
    id: "forge.importers",
    version: "0.1.0",
    engine: "^0.1",
    kind: Source,
    provides: [
        Importer("gltf"), Importer("image"), Importer("ktx2"),
        Exporter("gltf"),
    ],
    capabilities: [ Fs(ProjectRead), Fs(ProjectWrite) ],
)"#;

/// The first-party importers and exporter.
pub struct Importers {
    manifest: Manifest,
}

impl Importers {
    /// The plugin.
    pub fn new() -> Result<Self, PluginError> {
        Ok(Self {
            manifest: Manifest::parse(MANIFEST)?,
        })
    }
}

impl SourcePlugin for Importers {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        let imp = |i: Arc<dyn Importer>| i;
        cx.add::<ImporterPoint>("gltf", imp(Arc::new(GltfImporter)), Order::Last)?;
        cx.add::<ImporterPoint>("image", imp(Arc::new(ImageImporter)), Order::Last)?;
        cx.add::<ImporterPoint>("ktx2", imp(Arc::new(Ktx2Importer)), Order::Last)?;
        cx.add::<ExporterPoint>("gltf", Arc::new(GltfExporter), Order::Last)
    }
}

/// The asset points with the kernel's asset types (`forge.asset`) and these importers
/// (`forge.importers`) loaded through the ordinary loader: the standard asset plugin set.
pub fn asset_extensions() -> Result<Extensions, AssetError> {
    let plug = |e: PluginError| AssetError::Import {
        path: String::new(),
        importer: "forge.importers".into(),
        why: e.to_string(),
    };
    let mut x = Extensions::new();
    AssetServer::define_points(&mut x).map_err(plug)?;
    let types = FirstPartyAssets::new().map_err(plug)?;
    let importers = Importers::new().map_err(plug)?;
    loader::load(&mut x, &[&types, &importers], &[], &Grants::new()).map_err(plug)?;
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_set_has_every_first_party_importer() {
        let x = asset_extensions().unwrap_or_else(|e| panic!("{e}"));
        let keys: Vec<&str> = x
            .registry::<ImporterPoint>()
            .map(|r| r.keys().collect())
            .unwrap_or_default();
        assert_eq!(keys, ["gltf", "image", "ktx2"]);
        assert!(
            x.registry::<ExporterPoint>()
                .and_then(|r| r.get("gltf"))
                .is_some()
        );
    }
}
