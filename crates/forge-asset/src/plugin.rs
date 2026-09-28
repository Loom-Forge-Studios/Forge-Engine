//! The kernel's asset types as a plugin, `forge.asset`: the four built-in asset types (mesh,
//! texture, material, scene), registered through the ordinary loader like any third-party
//! type (I16). The types are data the kernel defines; everything that *produces* them —
//! importers and exporters — lives in plugins (the first-party glTF, image and KTX2 ones are
//! `plugins/forge-importers`, `forge.importers`). A plugin can add `fbx`, replace `gltf`, or
//! chain it; the asset server sees only the registries.

use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

use crate::importer::{AssetTypeItem, AssetTypePoint};
use crate::types::{MaterialAsset, MeshAsset, SceneAsset, TextureAsset};

const MANIFEST: &str = r#"Plugin(
    id: "forge.asset",
    version: "0.2.0",
    engine: "^0.1",
    kind: Source,
    provides: [
        AssetType("mesh"), AssetType("texture"), AssetType("material"), AssetType("scene"),
    ],
)"#;

/// The kernel's asset types.
pub struct FirstPartyAssets {
    manifest: Manifest,
}

impl FirstPartyAssets {
    /// The plugin.
    pub fn new() -> Result<Self, PluginError> {
        Ok(Self {
            manifest: Manifest::parse(MANIFEST)?,
        })
    }
}

impl SourcePlugin for FirstPartyAssets {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<AssetTypePoint>("mesh", AssetTypeItem::of::<MeshAsset>(), Order::Last)?;
        cx.add::<AssetTypePoint>("texture", AssetTypeItem::of::<TextureAsset>(), Order::Last)?;
        cx.add::<AssetTypePoint>(
            "material",
            AssetTypeItem::of::<MaterialAsset>(),
            Order::Last,
        )?;
        cx.add::<AssetTypePoint>("scene", AssetTypeItem::of::<SceneAsset>(), Order::Last)
    }
}
