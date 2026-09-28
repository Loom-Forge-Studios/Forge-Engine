//! The texture artefact: a standard KTX2 container (Khronos KTX 2.0), so the artefact is
//! directly usable by any KTX2 tool and uploads to the GPU without conversion.
//!
//! Written here (the `ktx2` crate reads only): `VK_FORMAT_R8G8B8A8_SRGB` or `_UNORM`, one
//! face, one layer, a full mip chain, no supercompression, a basic data-format descriptor,
//! mip data stored smallest level first with 4-byte alignment, as the specification requires.
//! Block compression (BC7 / ASTC via Basis Universal) is an `Unbuilt` follow-up: it belongs
//! with forge-gpu's format negotiation, and the container already carries it.

use bytes::Bytes;

use crate::types::TextureAsset;

const IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
const VK_R8G8B8A8_UNORM: u32 = 37;
const VK_R8G8B8A8_SRGB: u32 = 43;
const HEADER_LEN: usize = 80;
const LEVEL_INDEX_LEN: usize = 24;
/// Basic DFD block: 24-byte header + 4 samples of 16 bytes.
const DFD_BLOCK_LEN: usize = 24 + 4 * 16;

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn dfd(srgb: bool) -> Vec<u8> {
    let mut d = Vec::with_capacity(4 + DFD_BLOCK_LEN);
    put32(&mut d, (4 + DFD_BLOCK_LEN) as u32); // dfdTotalSize
    put32(&mut d, 0); // vendorId 0 (Khronos) | descriptorType 0 (basic)
    d.extend_from_slice(&2u16.to_le_bytes()); // versionNumber (KDF 1.3)
    d.extend_from_slice(&(DFD_BLOCK_LEN as u16).to_le_bytes()); // descriptorBlockSize
    d.push(1); // colorModel: RGBSDA
    d.push(1); // colorPrimaries: BT709
    d.push(if srgb { 2 } else { 1 }); // transferFunction: sRGB / linear
    d.push(0); // flags: straight alpha
    d.extend_from_slice(&[0, 0, 0, 0]); // texelBlockDimension (1x1x1x1, stored minus one)
    d.extend_from_slice(&[4, 0, 0, 0, 0, 0, 0, 0]); // bytesPlane0 = 4
    for (i, channel) in [0u8, 1, 2, 15].into_iter().enumerate() {
        d.extend_from_slice(&((i as u16) * 8).to_le_bytes()); // bitOffset
        d.push(7); // bitLength - 1
        // Alpha is always linear, flagged so in an sRGB texture.
        let linear = if srgb && channel == 15 { 0x10 } else { 0 };
        d.push(channel | linear);
        d.extend_from_slice(&[0, 0, 0, 0]); // samplePosition
        put32(&mut d, 0); // sampleLower
        put32(&mut d, 255); // sampleUpper
    }
    d
}

/// Encode an RGBA8 texture as KTX2 bytes (deterministic).
pub(crate) fn write_rgba8(t: &TextureAsset) -> Bytes {
    let n = t.levels.len().max(1);
    let dfd = dfd(t.srgb);
    let dfd_off = HEADER_LEN + n * LEVEL_INDEX_LEN;
    let mut data_off = dfd_off + dfd.len();
    // Levels are stored smallest first; each starts 4-aligned.
    let mut offsets = vec![0usize; t.levels.len()];
    let mut body = Vec::new();
    for (i, lv) in t.levels.iter().enumerate().rev() {
        let pad = (4 - (data_off + body.len()) % 4) % 4;
        body.extend(std::iter::repeat_n(0u8, pad));
        offsets[i] = data_off + body.len();
        body.extend_from_slice(lv);
    }
    data_off += body.len();
    let mut out = Vec::with_capacity(data_off);
    out.extend_from_slice(&IDENTIFIER);
    put32(
        &mut out,
        if t.srgb {
            VK_R8G8B8A8_SRGB
        } else {
            VK_R8G8B8A8_UNORM
        },
    );
    put32(&mut out, 1); // typeSize
    put32(&mut out, t.width);
    put32(&mut out, t.height);
    put32(&mut out, 0); // pixelDepth
    put32(&mut out, 0); // layerCount
    put32(&mut out, 1); // faceCount
    put32(&mut out, t.levels.len() as u32);
    put32(&mut out, 0); // supercompressionScheme
    put32(&mut out, dfd_off as u32);
    put32(&mut out, dfd.len() as u32);
    put32(&mut out, 0); // kvdByteOffset
    put32(&mut out, 0); // kvdByteLength
    put64(&mut out, 0); // sgdByteOffset
    put64(&mut out, 0); // sgdByteLength
    for (lv, off) in t.levels.iter().zip(&offsets) {
        put64(&mut out, *off as u64);
        put64(&mut out, lv.len() as u64);
        put64(&mut out, lv.len() as u64);
    }
    out.extend_from_slice(&dfd);
    out.extend_from_slice(&body);
    Bytes::from(out)
}

/// Decode a KTX2 RGBA8 texture (the artefact format, or an imported `.ktx2` of that format).
pub(crate) fn read_rgba8(bytes: &Bytes) -> Result<TextureAsset, String> {
    let r = ktx2::Reader::new(&bytes[..]).map_err(|e| format!("not a valid KTX2 file: {e:?}"))?;
    let h = r.header();
    let srgb = match h.format.map(|f| f.value()) {
        Some(VK_R8G8B8A8_SRGB) => true,
        Some(VK_R8G8B8A8_UNORM) => false,
        other => {
            return Err(format!(
                "KTX2 format {other:?} is not RGBA8 (only R8G8B8A8_SRGB/UNORM are decoded so far)"
            ));
        }
    };
    if h.supercompression_scheme.is_some() {
        return Err("supercompressed KTX2 is not decoded yet".into());
    }
    if h.pixel_depth > 1 || h.layer_count > 1 || h.face_count != 1 {
        return Err("only 2D, single-layer, single-face KTX2 textures are supported".into());
    }
    let (w, hgt) = (h.pixel_width, h.pixel_height.max(1));
    let mut levels = Vec::new();
    for (i, lv) in r.levels().enumerate() {
        let lw = (w >> i).max(1) as usize;
        let lh = (hgt >> i).max(1) as usize;
        if lv.data.len() != lw * lh * 4 {
            return Err(format!(
                "level {i} holds {} bytes, expected {lw}x{lh}x4",
                lv.data.len()
            ));
        }
        let start = lv.data.as_ptr() as usize - bytes.as_ptr() as usize;
        levels.push(bytes.slice(start..start + lv.data.len()));
    }
    if levels.is_empty() {
        return Err("KTX2 file has no levels".into());
    }
    Ok(TextureAsset {
        width: w,
        height: hgt,
        srgb,
        levels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ktx2_round_trips_through_the_reference_reader() {
        let l0 = Bytes::from((0..4 * 4 * 4).map(|i| i as u8).collect::<Vec<_>>());
        let l1 = Bytes::from(vec![9u8; 2 * 2 * 4]);
        let l2 = Bytes::from(vec![7u8; 4]);
        for srgb in [true, false] {
            let t = TextureAsset {
                width: 4,
                height: 4,
                srgb,
                levels: vec![l0.clone(), l1.clone(), l2.clone()],
            };
            let b = write_rgba8(&t);
            let r = ktx2::Reader::new(&b[..]).expect("the reference reader accepts it");
            assert_eq!(r.header().level_count, 3);
            assert_eq!(
                r.transfer_function().map(|f| f.value()),
                Some(if srgb { 2 } else { 1 })
            );
            assert_eq!(read_rgba8(&b), Ok(t));
        }
        assert!(read_rgba8(&Bytes::from_static(b"not a ktx2 file at all, clearly")).is_err());
    }
}
