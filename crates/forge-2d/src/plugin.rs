//! `forge.2d` — the 2D pipeline as a plugin (Ch.32: every capability arrives through the
//! ordinary loader, I16).
//!
//! Provides `AssetType("sprite_sheet")` ([`SpriteSheetAsset`]: a sheet's slicing and its
//! animations) and `Importer("aseprite")` (`.ase` / `.aseprite` -> the sheet's texture and a
//! sprite sheet). The 2D preset names `forge.2d` in its default plugin set (Ch.31 §31.6,
//! `presets/2d/workspace.ron`); like every plugin it loads under every preset (I15).

use std::sync::Arc;

use bytes::Bytes;
use forge_asset::types::AssetValue;
use forge_asset::{
    AssetError, AssetTypeItem, AssetTypePoint, ImportCx, Importer, ImporterPoint,
    texture_from_rgba8,
};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use serde::{Deserialize, Serialize};

use crate::anim::{FrameAnim, SheetSlice};
use crate::aseprite;

const MANIFEST: &str = r#"Plugin(
    id: "forge.2d",
    version: "0.1.0",
    engine: "^0.1",
    kind: Source,
    provides: [ AssetType("sprite_sheet"), Importer("aseprite") ],
)"#;

/// The plugin id.
pub const PLUGIN_ID: &str = "forge.2d";

/// A sprite sheet as an asset: its slicing, its animations, and the label of the texture
/// artefact (in the same import) that holds its pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpriteSheetAsset {
    pub texture: String,
    pub slice: SheetSlice,
    pub anims: Vec<FrameAnim>,
    /// What the importer could not honour (a tilemap layer, a blend mode).
    pub warnings: Vec<String>,
}

impl AssetValue for SpriteSheetAsset {
    const KIND: &'static str = "sprite_sheet";
    fn decode(bytes: &Bytes) -> Result<Self, String> {
        let s = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        ron::from_str(s).map_err(|e| e.to_string())
    }
    fn encode(&self) -> Result<Bytes, String> {
        // Plain RON with fields in declaration order: deterministic bytes. A failure fails
        // the import that emits it (`ImportCx::emit_value`), never an empty artefact.
        ron::to_string(self)
            .map(Bytes::from)
            .map_err(|e| e.to_string())
    }
}

/// The Aseprite importer. Settings: `mips` (default `false`: pixel art is sampled at whole
/// texels).
pub struct AsepriteImporter;

impl Importer for AsepriteImporter {
    fn version(&self) -> u32 {
        1
    }
    fn extensions(&self) -> &[&str] {
        &["ase", "aseprite"]
    }
    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        let doc = aseprite::decode(cx.source()).map_err(|e| cx.fail(e))?;
        let (sheet, slice, anims) = aseprite::to_sheet(&doc);
        let name = cx.path().file_name().to_string();
        let tex = texture_from_rgba8(sheet.rgba, sheet.w, sheet.h, true, cx.flag("mips", false));
        cx.emit_value("texture", &format!("{name} (texture)"), &tex)?;
        let asset = SpriteSheetAsset {
            texture: "texture".into(),
            slice,
            anims,
            warnings: doc.warnings,
        };
        cx.emit_value("", &name, &asset)?;
        Ok(())
    }
}

/// See the module docs.
pub struct Plugin2d {
    manifest: Manifest,
}

impl Plugin2d {
    /// The plugin.
    pub fn new() -> Result<Self, PluginError> {
        Ok(Self {
            manifest: Manifest::parse(MANIFEST)?,
        })
    }
}

impl SourcePlugin for Plugin2d {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<AssetTypePoint>(
            "sprite_sheet",
            AssetTypeItem::of::<SpriteSheetAsset>(),
            Order::Last,
        )?;
        let imp: Arc<dyn Importer> = Arc::new(AsepriteImporter);
        cx.add::<ImporterPoint>("aseprite", imp, Order::Last)
    }
}
