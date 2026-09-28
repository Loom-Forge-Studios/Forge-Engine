//! Image importers: PNG / JPEG -> a KTX2 RGBA8 texture with a full mip chain (built by
//! `forge_asset::texture_from_rgba8`, so it hashes like every other texture), and `.ktx2`
//! passthrough (validated).
//!
//! Settings: `srgb` (`true`: colour; `false`: data such as normal maps; default `true`),
//! `mips` (default `true`).

use forge_asset::types::{AssetValue, KIND_TEXTURE, TextureAsset};
use forge_asset::{AssetError, ImportCx, Importer, texture_from_rgba8};

/// Decode PNG/JPEG bytes to a texture.
pub fn decode_image(bytes: &[u8], srgb: bool, mips: bool) -> Result<TextureAsset, String> {
    let fmt = image::guess_format(bytes).map_err(|e| e.to_string())?;
    if !matches!(fmt, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
        return Err(format!(
            "{fmt:?} images are not supported (PNG, JPEG, KTX2)"
        ));
    }
    let img = image::load_from_memory_with_format(bytes, fmt).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err("the image is empty".into());
    }
    Ok(texture_from_rgba8(rgba.into_raw(), w, h, srgb, mips))
}

/// PNG / JPEG importer (`image`).
pub struct ImageImporter;

impl Importer for ImageImporter {
    fn version(&self) -> u32 {
        1
    }
    fn extensions(&self) -> &[&str] {
        &["png", "jpg", "jpeg"]
    }
    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        let (srgb, mips) = (cx.flag("srgb", true), cx.flag("mips", true));
        let tex = decode_image(cx.source(), srgb, mips).map_err(|e| cx.fail(e))?;
        let name = cx.path().file_name().to_string();
        cx.emit_value("", &name, &tex)?;
        Ok(())
    }
}

/// `.ktx2` importer: validated passthrough (the artefact *is* the file).
pub struct Ktx2Importer;

impl Importer for Ktx2Importer {
    fn version(&self) -> u32 {
        1
    }
    fn extensions(&self) -> &[&str] {
        &["ktx2"]
    }
    fn import(&self, cx: &mut ImportCx<'_>) -> Result<(), AssetError> {
        // Validate only: the file is already a valid artefact, byte for byte.
        TextureAsset::decode(cx.source()).map_err(|e| cx.fail(e))?;
        let name = cx.path().file_name().to_string();
        let bytes = cx.source().clone();
        cx.emit("", KIND_TEXTURE, &name, bytes);
        Ok(())
    }
}
