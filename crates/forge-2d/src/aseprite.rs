//! Aseprite import (Ch.35 §35.2): `.ase` / `.aseprite` files read directly — no export step.
//!
//! [`decode`] reads the file format (header, frames, layer, cel, tag and palette chunks;
//! RGBA, grayscale and indexed colour; raw, linked and zlib-compressed cels; per-frame
//! durations) and composites each frame's visible layers with their opacity. [`to_sheet`]
//! lays the frames out as a sprite sheet with one animation per **tag** (forward, reverse,
//! ping-pong) and per-frame durations — the importer ([`crate::plugin`],
//! `Importer("aseprite")`) emits the sheet's texture and the animations as assets.
//!
//! Untrusted input: every read is bounds-checked, sizes are capped
//! ([`MAX_PIXELS`]) before anything is allocated, and decompression is limited to the size
//! the cel declares. A malformed file is `TWOD-0003`, never a panic.
//!
//! What is not composited is reported, not silently dropped: tilemap layers and blend modes
//! other than Normal come back in [`AseDoc::warnings`] (the blend is composited as Normal).
//!
//! [`write`] is the inverse for 32-bit files (tests build fixtures with it; it is also a
//! lossless way to hand a sheet back to an artist).

use crate::Error2d;
use crate::anim::{FrameAnim, Playback, SheetSlice};
use crate::atlas::Image;

/// Largest canvas accepted (pixels, per frame).
pub const MAX_PIXELS: u64 = 1 << 24;
const MAX_FRAMES: usize = 4096;

/// A layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AseLayer {
    pub name: String,
    pub visible: bool,
    /// 0 normal image, 1 group, 2 tilemap.
    pub kind: u16,
    pub child_level: u16,
    pub blend: u16,
    pub opacity: u8,
}

/// A tag (a named frame range: an animation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AseTag {
    pub name: String,
    pub from: u16,
    pub to: u16,
    /// 0 forward, 1 reverse, 2 ping-pong, 3 ping-pong reverse.
    pub direction: u8,
}

/// A decoded file: composited frames, durations, layers and tags.
#[derive(Clone, Debug, PartialEq)]
pub struct AseDoc {
    pub w: u32,
    pub h: u32,
    pub frames: Vec<Image>,
    pub durations_ms: Vec<u16>,
    pub layers: Vec<AseLayer>,
    pub tags: Vec<AseTag>,
    pub warnings: Vec<String>,
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn need(&self, n: usize) -> Result<(), Error2d> {
        if self.p.checked_add(n).is_none_or(|e| e > self.b.len()) {
            return Err(Error2d::aseprite(format!("truncated at byte {}", self.p)));
        }
        Ok(())
    }
    fn u8(&mut self) -> Result<u8, Error2d> {
        self.need(1)?;
        let v = self.b[self.p];
        self.p += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, Error2d> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.b[self.p], self.b[self.p + 1]]);
        self.p += 2;
        Ok(v)
    }
    fn i16(&mut self) -> Result<i16, Error2d> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32, Error2d> {
        self.need(4)?;
        let v = u32::from_le_bytes([
            self.b[self.p],
            self.b[self.p + 1],
            self.b[self.p + 2],
            self.b[self.p + 3],
        ]);
        self.p += 4;
        Ok(v)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error2d> {
        self.need(n)?;
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<(), Error2d> {
        self.bytes(n).map(|_| ())
    }
    fn string(&mut self) -> Result<String, Error2d> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
}

/// A cel as read: which layer, where, and its pixels as RGBA (or a link).
enum CelData {
    Pixels(u32, u32, Vec<u8>),
    Link(u16),
}

struct Cel {
    layer: u16,
    x: i16,
    y: i16,
    opacity: u8,
    z: i16,
    data: CelData,
}

/// `(r, g, b, a)` of pixel data in the file's colour depth.
fn to_rgba(
    depth: u16,
    raw: &[u8],
    w: u32,
    h: u32,
    palette: &[[u8; 4]],
    transparent: u8,
) -> Result<Vec<u8>, Error2d> {
    let n = (w as usize) * (h as usize);
    let bpp = usize::from(depth / 8);
    if raw.len() < n * bpp {
        return Err(Error2d::aseprite(format!(
            "a {w}x{h} cel has {} bytes, needs {}",
            raw.len(),
            n * bpp
        )));
    }
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        match depth {
            32 => out.extend_from_slice(&raw[i * 4..i * 4 + 4]),
            16 => {
                let (v, a) = (raw[i * 2], raw[i * 2 + 1]);
                out.extend_from_slice(&[v, v, v, a]);
            }
            8 => {
                let idx = raw[i];
                if idx == transparent {
                    out.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    let c = palette
                        .get(usize::from(idx))
                        .copied()
                        .unwrap_or([0, 0, 0, 255]);
                    out.extend_from_slice(&c);
                }
            }
            d => {
                return Err(Error2d::aseprite(format!(
                    "colour depth {d} is not 8, 16 or 32"
                )));
            }
        }
    }
    Ok(out)
}

/// Straight-alpha "over" with an extra opacity, in integers (round half up).
fn over(dst: &mut [u8], src: [u8; 4], opacity: u32) {
    let sa = (u32::from(src[3]) * opacity + 127) / 255;
    if sa == 0 {
        return;
    }
    let da = u32::from(dst[3]);
    let inv = 255 - sa;
    let oa = sa + (da * inv + 127) / 255;
    if oa == 0 {
        return;
    }
    for k in 0..3 {
        let c = u32::from(src[k]) * sa * 255 + u32::from(dst[k]) * da * inv;
        dst[k] = ((c + oa * 255 / 2) / (oa * 255)).min(255) as u8;
    }
    dst[3] = oa.min(255) as u8;
}

/// Decode an Aseprite file (see the module docs).
pub fn decode(bytes: &[u8]) -> Result<AseDoc, Error2d> {
    let mut r = Reader { b: bytes, p: 0 };
    let _size = r.u32()?;
    if r.u16()? != 0xA5E0 {
        return Err(Error2d::aseprite("not an Aseprite file (bad magic)"));
    }
    let nframes = usize::from(r.u16()?);
    let (w, h) = (u32::from(r.u16()?), u32::from(r.u16()?));
    let depth = r.u16()?;
    let flags = r.u32()?;
    r.skip(2 + 4 + 4)?;
    let transparent = r.u8()?;
    r.skip(3)?;
    let _ncolors = r.u16()?;
    r.skip(2 + 2 * 2 + 2 * 2 + 84)?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(Error2d::aseprite(format!(
            "canvas {w}x{h} is empty or larger than {MAX_PIXELS} pixels"
        )));
    }
    if ![8, 16, 32].contains(&depth) {
        return Err(Error2d::aseprite(format!(
            "colour depth {depth} is not 8, 16 or 32"
        )));
    }
    if nframes == 0 || nframes > MAX_FRAMES {
        return Err(Error2d::aseprite(format!(
            "{nframes} frames (1 to {MAX_FRAMES} supported)"
        )));
    }
    let layer_opacity_valid = flags & 1 != 0;
    let mut layers: Vec<AseLayer> = Vec::new();
    let mut tags = Vec::new();
    let mut palette: Vec<[u8; 4]> = vec![[0, 0, 0, 255]; 256];
    let mut warnings = Vec::new();
    let mut frame_cels: Vec<Vec<Cel>> = Vec::with_capacity(nframes);
    let mut durations = Vec::with_capacity(nframes);
    for f in 0..nframes {
        let start = r.p;
        let fbytes = r.u32()? as usize;
        if r.u16()? != 0xF1FA {
            return Err(Error2d::aseprite(format!("frame {f}: bad frame magic")));
        }
        let old_chunks = r.u16()?;
        durations.push(r.u16()?);
        r.skip(2)?;
        let new_chunks = r.u32()?;
        let chunks = if new_chunks == 0 {
            u32::from(old_chunks)
        } else {
            new_chunks
        };
        let frame_end = start
            .checked_add(fbytes)
            .filter(|e| *e <= bytes.len() && fbytes >= 16)
            .ok_or_else(|| {
                Error2d::aseprite(format!("frame {f}: size {fbytes} runs past the file"))
            })?;
        let mut cels = Vec::new();
        for _ in 0..chunks {
            let cstart = r.p;
            let csize = r.u32()? as usize;
            let ctype = r.u16()?;
            let cend = cstart
                .checked_add(csize)
                .filter(|e| *e <= frame_end && csize >= 6)
                .ok_or_else(|| {
                    Error2d::aseprite(format!("frame {f}: a chunk runs past its frame"))
                })?;
            let mut c = Reader {
                b: &bytes[..cend],
                p: r.p,
            };
            match ctype {
                0x2004 => {
                    let lflags = c.u16()?;
                    let kind = c.u16()?;
                    let child_level = c.u16()?;
                    c.skip(4)?;
                    let blend = c.u16()?;
                    let op = c.u8()?;
                    c.skip(3)?;
                    let name = c.string()?;
                    layers.push(AseLayer {
                        name,
                        visible: lflags & 1 != 0,
                        kind,
                        child_level,
                        blend,
                        opacity: if layer_opacity_valid { op } else { 255 },
                    });
                }
                0x2005 => {
                    let layer = c.u16()?;
                    let x = c.i16()?;
                    let y = c.i16()?;
                    let opacity = c.u8()?;
                    let ctype2 = c.u16()?;
                    let z = c.i16()?;
                    c.skip(5)?;
                    let data = match ctype2 {
                        0 | 2 => {
                            let (cw, ch) = (u32::from(c.u16()?), u32::from(c.u16()?));
                            if u64::from(cw) * u64::from(ch) > MAX_PIXELS {
                                return Err(Error2d::aseprite("a cel is too large"));
                            }
                            let need = cw as usize * ch as usize * usize::from(depth / 8);
                            let raw = if ctype2 == 0 {
                                c.bytes(need)?.to_vec()
                            } else {
                                let z = &bytes[c.p..cend];
                                miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(z, need)
                                    .map_err(|e| {
                                        Error2d::aseprite(format!(
                                            "frame {f}: a compressed cel does not inflate: {e:?}"
                                        ))
                                    })?
                            };
                            CelData::Pixels(
                                cw,
                                ch,
                                to_rgba(depth, &raw, cw, ch, &palette, transparent)?,
                            )
                        }
                        1 => CelData::Link(c.u16()?),
                        3 => {
                            warnings.push(format!(
                                "frame {f}: a tilemap cel on layer {layer} is not composited"
                            ));
                            r.p = cend;
                            continue;
                        }
                        t => {
                            return Err(Error2d::aseprite(format!(
                                "frame {f}: cel type {t} is unknown"
                            )));
                        }
                    };
                    cels.push(Cel {
                        layer,
                        x,
                        y,
                        opacity,
                        z,
                        data,
                    });
                }
                0x2018 => {
                    let n = c.u16()?;
                    c.skip(8)?;
                    for _ in 0..n {
                        let from = c.u16()?;
                        let to = c.u16()?;
                        let direction = c.u8()?;
                        c.skip(2 + 6 + 3 + 1)?;
                        let name = c.string()?;
                        tags.push(AseTag {
                            name,
                            from,
                            to,
                            direction,
                        });
                    }
                }
                0x2019 => {
                    let size = c.u32()? as usize;
                    let first = c.u32()? as usize;
                    let last = c.u32()? as usize;
                    c.skip(8)?;
                    if palette.len() < size.min(256) {
                        palette.resize(size.min(256), [0, 0, 0, 255]);
                    }
                    for i in first..=last.min(first + 255) {
                        let eflags = c.u16()?;
                        let rgba = [c.u8()?, c.u8()?, c.u8()?, c.u8()?];
                        if eflags & 1 != 0 {
                            let _ = c.string()?;
                        }
                        if let Some(p) = palette.get_mut(i) {
                            *p = rgba;
                        }
                    }
                }
                0x0004 | 0x0011 => {
                    let packets = c.u16()?;
                    let mut idx = 0usize;
                    for _ in 0..packets {
                        idx += usize::from(c.u8()?);
                        let n = match c.u8()? {
                            0 => 256,
                            n => usize::from(n),
                        };
                        for _ in 0..n {
                            let mut rgb = [c.u8()?, c.u8()?, c.u8()?];
                            if ctype == 0x0011 {
                                for v in &mut rgb {
                                    *v = ((u32::from(*v) * 255 + 31) / 63).min(255) as u8;
                                }
                            }
                            if let Some(p) = palette.get_mut(idx) {
                                *p = [rgb[0], rgb[1], rgb[2], 255];
                            }
                            idx += 1;
                        }
                    }
                }
                _ => {} // colour profile, slices, user data, external files: not needed
            }
            r.p = cend;
        }
        r.p = frame_end;
        frame_cels.push(cels);
    }
    // Which layers show: visible, not a group, and every enclosing group visible.
    let mut shown = vec![false; layers.len()];
    let mut stack: Vec<(u16, bool)> = Vec::new();
    for (i, l) in layers.iter().enumerate() {
        while stack.last().is_some_and(|(lvl, _)| *lvl >= l.child_level) {
            stack.pop();
        }
        let parents_visible = stack.iter().all(|(_, v)| *v);
        let vis = l.visible && parents_visible;
        if l.kind == 1 {
            stack.push((l.child_level, vis));
        } else if l.kind == 2 {
            warnings.push(format!(
                "layer {:?} is a tilemap layer and is not composited",
                l.name
            ));
        } else {
            shown[i] = vis;
            if vis && l.blend != 0 {
                warnings.push(format!(
                    "layer {:?} uses blend mode {}; composited as Normal",
                    l.name, l.blend
                ));
            }
        }
    }
    // Composite.
    let mut frames = Vec::with_capacity(nframes);
    for f in 0..nframes {
        let mut canvas = Image::filled(w, h, [0, 0, 0, 0]);
        let mut order: Vec<&Cel> = frame_cels[f].iter().collect();
        order.sort_by_key(|c| (i32::from(c.layer) + i32::from(c.z), c.z));
        for cel in order {
            let li = usize::from(cel.layer);
            if !shown.get(li).copied().unwrap_or(false) {
                continue;
            }
            let (cw, ch, px) = match &cel.data {
                CelData::Pixels(cw, ch, px) => (*cw, *ch, px),
                CelData::Link(to) => {
                    let Some(src) = frame_cels
                        .get(usize::from(*to))
                        .and_then(|cs| cs.iter().find(|c| c.layer == cel.layer))
                    else {
                        continue;
                    };
                    match &src.data {
                        CelData::Pixels(cw, ch, px) => (*cw, *ch, px),
                        CelData::Link(_) => continue,
                    }
                }
            };
            let opacity = u32::from(cel.opacity) * u32::from(layers[li].opacity) / 255;
            for yy in 0..ch {
                let ty = i64::from(cel.y) + i64::from(yy);
                if ty < 0 || ty >= i64::from(h) {
                    continue;
                }
                for xx in 0..cw {
                    let tx = i64::from(cel.x) + i64::from(xx);
                    if tx < 0 || tx >= i64::from(w) {
                        continue;
                    }
                    let si = ((yy * cw + xx) * 4) as usize;
                    let s = [px[si], px[si + 1], px[si + 2], px[si + 3]];
                    let di = ((ty as u32 * w + tx as u32) * 4) as usize;
                    over(&mut canvas.rgba[di..di + 4], s, opacity);
                }
            }
        }
        frames.push(canvas);
    }
    Ok(AseDoc {
        w,
        h,
        frames,
        durations_ms: durations,
        layers,
        tags,
        warnings,
    })
}

/// The grid [`to_sheet`] lays `frames` frames out on: `(columns, rows)`, near-square,
/// row-major. [`crate::atlas::image_file_size`] answers an Aseprite file's sheet size with it.
#[must_use]
pub fn sheet_grid(frames: u32) -> (u32, u32) {
    let mut cols = 1;
    while cols * cols < frames {
        cols += 1;
    }
    (cols, frames.div_ceil(cols))
}

/// Lay the frames out as a sheet (a near-square grid, row-major: [`sheet_grid`]) with its
/// slicing and one animation per tag (`"default"`: every frame, when the file has no tags).
#[must_use]
pub fn to_sheet(doc: &AseDoc) -> (Image, SheetSlice, Vec<FrameAnim>) {
    let n = doc.frames.len() as u32;
    let (cols, rows) = sheet_grid(n);
    let mut sheet = Image::filled(cols * doc.w, rows * doc.h, [0, 0, 0, 0]);
    for (i, f) in doc.frames.iter().enumerate() {
        let (cx, cy) = (i as u32 % cols, i as u32 / cols);
        for y in 0..doc.h {
            let src = ((y * doc.w) * 4) as usize;
            let dst = (((cy * doc.h + y) * sheet.w + cx * doc.w) * 4) as usize;
            sheet.rgba[dst..dst + (doc.w * 4) as usize]
                .copy_from_slice(&f.rgba[src..src + (doc.w * 4) as usize]);
        }
    }
    let slice = SheetSlice {
        image_w: sheet.w,
        image_h: sheet.h,
        cell_w: doc.w,
        cell_h: doc.h,
        ..SheetSlice::default()
    };
    let dur = |i: u32| u64::from(doc.durations_ms.get(i as usize).copied().unwrap_or(100)) * 1000;
    let mut anims = Vec::new();
    if doc.tags.is_empty() {
        anims.push(FrameAnim {
            name: "default".into(),
            frames: (0..n).collect(),
            durations_us: (0..n).map(dur).collect(),
            playback: Playback::Loop,
        });
    }
    for t in &doc.tags {
        let (a, b) = (
            u32::from(t.from.min(t.to)),
            u32::from(t.to.max(t.from)).min(n.saturating_sub(1)),
        );
        let mut frames: Vec<u32> = (a..=b).collect();
        let playback = match t.direction {
            1 => Playback::Reverse,
            2 => Playback::PingPong,
            3 => {
                frames.reverse();
                Playback::PingPong
            }
            _ => Playback::Loop,
        };
        let durations_us = frames.iter().map(|f| dur(*f)).collect();
        anims.push(FrameAnim {
            name: t.name.clone(),
            frames,
            durations_us,
            playback,
        });
    }
    (sheet, slice, anims)
}

/// A cel to write: `(layer, x, y, pixels)`.
pub type WriteCel = (u16, i16, i16, Image);

/// A 32-bit file to write: layers, frames of `(layer, x, y, image)` cels, tags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AseWrite {
    pub w: u16,
    pub h: u16,
    /// `(name, visible, opacity)`.
    pub layers: Vec<(String, bool, u8)>,
    /// `(duration ms, cels)`.
    pub frames: Vec<(u16, Vec<WriteCel>)>,
    pub tags: Vec<AseTag>,
    /// zlib-compress cels (type 2) instead of raw (type 0).
    pub compress: bool,
}

/// Encode `doc` as an Aseprite file (see the module docs).
#[must_use]
pub fn write(doc: &AseWrite) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: u16, body: &[u8]) {
        out.extend_from_slice(&((body.len() + 6) as u32).to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(body);
    }
    fn string(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u16).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }
    let mut out = vec![0u8; 128];
    out[4..6].copy_from_slice(&0xA5E0u16.to_le_bytes());
    out[6..8].copy_from_slice(&(doc.frames.len() as u16).to_le_bytes());
    out[8..10].copy_from_slice(&doc.w.to_le_bytes());
    out[10..12].copy_from_slice(&doc.h.to_le_bytes());
    out[12..14].copy_from_slice(&32u16.to_le_bytes());
    out[14..18].copy_from_slice(&1u32.to_le_bytes()); // layer opacity valid
    out[34] = 1; // pixel width
    out[35] = 1; // pixel height
    for (fi, (dur, cels)) in doc.frames.iter().enumerate() {
        let mut body = Vec::new();
        let mut chunks = 0u32;
        if fi == 0 {
            for (name, visible, opacity) in &doc.layers {
                let mut b = Vec::new();
                b.extend_from_slice(&(u16::from(*visible)).to_le_bytes());
                b.extend_from_slice(&0u16.to_le_bytes()); // image layer
                b.extend_from_slice(&0u16.to_le_bytes()); // child level
                b.extend_from_slice(&[0; 4]);
                b.extend_from_slice(&0u16.to_le_bytes()); // normal blend
                b.push(*opacity);
                b.extend_from_slice(&[0; 3]);
                string(&mut b, name);
                chunk(&mut body, 0x2004, &b);
                chunks += 1;
            }
            if !doc.tags.is_empty() {
                let mut b = Vec::new();
                b.extend_from_slice(&(doc.tags.len() as u16).to_le_bytes());
                b.extend_from_slice(&[0; 8]);
                for t in &doc.tags {
                    b.extend_from_slice(&t.from.to_le_bytes());
                    b.extend_from_slice(&t.to.to_le_bytes());
                    b.push(t.direction);
                    b.extend_from_slice(&[0; 2 + 6 + 3 + 1]);
                    string(&mut b, &t.name);
                }
                chunk(&mut body, 0x2018, &b);
                chunks += 1;
            }
        }
        for (layer, x, y, img) in cels {
            let mut b = Vec::new();
            b.extend_from_slice(&layer.to_le_bytes());
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
            b.push(255);
            b.extend_from_slice(&(if doc.compress { 2u16 } else { 0u16 }).to_le_bytes());
            b.extend_from_slice(&0i16.to_le_bytes());
            b.extend_from_slice(&[0; 5]);
            b.extend_from_slice(&(img.w as u16).to_le_bytes());
            b.extend_from_slice(&(img.h as u16).to_le_bytes());
            if doc.compress {
                b.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&img.rgba, 6));
            } else {
                b.extend_from_slice(&img.rgba);
            }
            chunk(&mut body, 0x2005, &b);
            chunks += 1;
        }
        let mut fh = Vec::new();
        fh.extend_from_slice(&((body.len() + 16) as u32).to_le_bytes());
        fh.extend_from_slice(&0xF1FAu16.to_le_bytes());
        fh.extend_from_slice(&(chunks.min(0xFFFF) as u16).to_le_bytes());
        fh.extend_from_slice(&dur.to_le_bytes());
        fh.extend_from_slice(&[0; 2]);
        fh.extend_from_slice(&chunks.to_le_bytes());
        out.extend_from_slice(&fh);
        out.extend_from_slice(&body);
    }
    let size = out.len() as u32;
    out[0..4].copy_from_slice(&size.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32, a: [u8; 4], b: [u8; 4]) -> Image {
        let mut img = Image::filled(w, h, a);
        for y in 0..h {
            for x in 0..w {
                if (x + y) % 2 == 1 {
                    img.set(x, y, b);
                }
            }
        }
        img
    }

    fn sample(compress: bool) -> AseWrite {
        AseWrite {
            w: 8,
            h: 6,
            layers: vec![
                ("bg".into(), true, 255),
                ("fx".into(), true, 128),
                ("hidden".into(), false, 255),
            ],
            frames: vec![
                (
                    100,
                    vec![
                        (0, 0, 0, checker(8, 6, [255, 0, 0, 255], [0, 0, 255, 255])),
                        (1, 2, 1, Image::filled(2, 2, [0, 255, 0, 255])),
                    ],
                ),
                (
                    150,
                    vec![
                        (0, 0, 0, Image::filled(8, 6, [10, 20, 30, 255])),
                        (2, 0, 0, Image::filled(8, 6, [255, 255, 255, 255])),
                    ],
                ),
                (50, vec![(0, -2, 3, Image::filled(4, 4, [9, 9, 9, 255]))]),
            ],
            tags: vec![
                AseTag {
                    name: "run".into(),
                    from: 0,
                    to: 1,
                    direction: 0,
                },
                AseTag {
                    name: "bob".into(),
                    from: 0,
                    to: 2,
                    direction: 2,
                },
            ],
            compress,
        }
    }

    #[test]
    fn a_written_file_decodes_to_its_composite_raw_and_compressed() {
        for compress in [false, true] {
            let doc = decode(&write(&sample(compress))).expect("decode");
            assert_eq!((doc.w, doc.h, doc.frames.len()), (8, 6, 3));
            assert_eq!(doc.durations_ms, vec![100, 150, 50]);
            assert_eq!(doc.frames[0].get(0, 0), [255, 0, 0, 255]);
            assert_eq!(doc.frames[0].get(1, 0), [0, 0, 255, 255]);
            // Half-opaque green over red at (2, 1)... (x+y)=3 odd -> blue under it.
            let p = doc.frames[0].get(2, 1);
            assert_eq!(p[3], 255);
            assert!(
                (i32::from(p[1]) - 128).abs() <= 1 && (i32::from(p[2]) - 127).abs() <= 1,
                "{p:?}"
            );
            assert_eq!(
                doc.frames[1].get(4, 4),
                [10, 20, 30, 255],
                "the hidden layer does not show"
            );
            assert_eq!(
                doc.frames[2].get(0, 3),
                [9, 9, 9, 255],
                "a cel partly off the canvas is clipped"
            );
            assert_eq!(doc.frames[2].get(3, 3), [0, 0, 0, 0]);
            assert_eq!(doc.tags.len(), 2);
            assert!(doc.warnings.is_empty());
        }
    }

    #[test]
    fn sheets_and_tag_animations() {
        let doc = decode(&write(&sample(true))).expect("decode");
        let (sheet, slice, anims) = to_sheet(&doc);
        assert_eq!((sheet.w, sheet.h), (16, 12));
        assert_eq!(crate::anim::slice(&slice).len(), 4);
        assert_eq!(sheet.get(8, 0), doc.frames[1].get(0, 0));
        let run = anims.iter().find(|a| a.name == "run").expect("run");
        assert_eq!(run.frames, vec![0, 1]);
        assert_eq!(run.durations_us, vec![100_000, 150_000]);
        let bob = anims.iter().find(|a| a.name == "bob").expect("bob");
        assert_eq!(bob.playback, Playback::PingPong);
        assert_eq!(bob.frame_at(260_000), Some(2));
        assert_eq!(bob.frame_at(300_000), Some(1), "then back down");
    }

    #[test]
    fn malformed_files_are_errors_never_panics() {
        let good = write(&sample(true));
        for cut in [0, 3, 10, 127, 128, 140, good.len() / 2, good.len() - 1] {
            let e = decode(&good[..cut]).expect_err("truncated");
            assert_eq!(e.code().as_str(), "TWOD-0003", "cut at {cut}");
        }
        let mut bad = good.clone();
        bad[4] = 0;
        assert!(decode(&bad).is_err());
        // Flip bytes everywhere: never a panic.
        for i in (0..good.len()).step_by(7) {
            let mut f = good.clone();
            f[i] ^= 0xA5;
            let _ = decode(&f);
        }
        // A huge declared canvas is refused before allocating.
        let mut huge = good;
        huge[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        huge[10..12].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(decode(&huge).is_err());
    }
}
