//! Atlas packing (Ch.35 §35.2 "sprite batcher with atlas packing").
//!
//! Sprites are packed into as few fixed-size pages as possible with **MaxRects** (best short
//! side fit, Jylänki 2010), each with a transparent `padding` gutter and its edge pixels
//! `extrude`d into the gutter so bilinear sampling and sub-pixel camera motion never bleed a
//! neighbour's colour. Fewer pages is fewer texture switches, which is what lets the batcher
//! ([`crate::sprite`]) draw a whole level in a handful of draw calls.
//!
//! Packing is deterministic: images are placed in a fixed order (longest side, then area,
//! then key) and every tie in the free-rectangle search is broken by position, so the same
//! inputs always give the same atlas (it joins the I2 corpus).

use std::collections::BTreeMap;

use crate::Error2d;

/// A placed image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    /// The page it is on.
    pub page: u32,
    /// Its pixels' top-left corner in the page (inside the gutter).
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Rect {
    fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x
            && o.y >= self.y
            && o.x + o.w <= self.x + self.w
            && o.y + o.h <= self.y + self.h
    }
    fn intersects(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// One page's free space.
#[derive(Clone, Debug)]
struct MaxRects {
    free: Vec<Rect>,
}

impl MaxRects {
    fn new(w: u32, h: u32) -> Self {
        Self {
            free: vec![Rect { x: 0, y: 0, w, h }],
        }
    }

    /// Best short side fit: the free rectangle leaving the smallest leftover on its shorter
    /// side; ties by the longer side, then top-most, then left-most.
    fn find(&self, w: u32, h: u32) -> Option<Rect> {
        let mut best: Option<(u32, u32, u32, u32, Rect)> = None;
        for f in &self.free {
            if f.w >= w && f.h >= h {
                let (dw, dh) = (f.w - w, f.h - h);
                let key = (dw.min(dh), dw.max(dh), f.y, f.x);
                let cand = Rect {
                    x: f.x,
                    y: f.y,
                    w,
                    h,
                };
                if best.is_none_or(|b| key < (b.0, b.1, b.2, b.3)) {
                    best = Some((key.0, key.1, key.2, key.3, cand));
                }
            }
        }
        best.map(|b| b.4)
    }

    fn place(&mut self, used: Rect) {
        let mut next = Vec::with_capacity(self.free.len() + 4);
        for f in &self.free {
            if !f.intersects(&used) {
                next.push(*f);
                continue;
            }
            // Split the free rectangle around the used one (up to four maximal pieces).
            if used.x > f.x {
                next.push(Rect {
                    x: f.x,
                    y: f.y,
                    w: used.x - f.x,
                    h: f.h,
                });
            }
            if used.x + used.w < f.x + f.w {
                let x = used.x + used.w;
                next.push(Rect {
                    x,
                    y: f.y,
                    w: f.x + f.w - x,
                    h: f.h,
                });
            }
            if used.y > f.y {
                next.push(Rect {
                    x: f.x,
                    y: f.y,
                    w: f.w,
                    h: used.y - f.y,
                });
            }
            if used.y + used.h < f.y + f.h {
                let y = used.y + used.h;
                next.push(Rect {
                    x: f.x,
                    y,
                    w: f.w,
                    h: f.y + f.h - y,
                });
            }
        }
        // Prune rectangles contained in another (keep the first of equal ones).
        let mut keep = vec![true; next.len()];
        for i in 0..next.len() {
            if !keep[i] {
                continue;
            }
            for j in 0..next.len() {
                if i != j && keep[j] && next[j].contains(&next[i]) && (next[i] != next[j] || j < i)
                {
                    keep[i] = false;
                    break;
                }
            }
        }
        self.free = next
            .into_iter()
            .zip(keep)
            .filter_map(|(r, k)| k.then_some(r))
            .collect();
    }
}

/// How to pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasOptions {
    /// Page width and height in pixels.
    pub page_w: u32,
    pub page_h: u32,
    /// Transparent-or-extruded pixels around every image.
    pub padding: u32,
    /// Copy each image's edge pixels this far into its gutter (at most `padding`).
    pub extrude: u32,
}

impl Default for AtlasOptions {
    fn default() -> Self {
        Self {
            page_w: 1024,
            page_h: 1024,
            padding: 2,
            extrude: 1,
        }
    }
}

/// Pack rectangles `(key, w, h)`: where each goes (no pixels). Fails if one cannot fit on
/// an empty page.
pub fn pack_rects(
    sizes: &[(u32, u32, u32)],
    opts: &AtlasOptions,
) -> Result<(BTreeMap<u32, Region>, u32), Error2d> {
    let pad = opts.padding;
    let mut order: Vec<&(u32, u32, u32)> = sizes.iter().collect();
    order.sort_by(|a, b| {
        let (ka, kb) = (a.1.max(a.2), b.1.max(b.2));
        kb.cmp(&ka)
            .then((b.1 * b.2).cmp(&(a.1 * a.2)))
            .then(a.0.cmp(&b.0))
    });
    let mut pages: Vec<MaxRects> = Vec::new();
    let mut out = BTreeMap::new();
    for &&(key, w, h) in &order {
        if w == 0 || h == 0 {
            return Err(Error2d::invalid(format!("image {key} has a zero size")));
        }
        let (pw, ph) = (w + 2 * pad, h + 2 * pad);
        if pw > opts.page_w || ph > opts.page_h {
            return Err(Error2d::AtlasFull {
                why: format!(
                    "image {key} ({w}x{h} + {pad} px padding) is larger than a {}x{} page",
                    opts.page_w, opts.page_h
                ),
            });
        }
        let mut placed = None;
        for (i, p) in pages.iter_mut().enumerate() {
            if let Some(r) = p.find(pw, ph) {
                p.place(r);
                placed = Some((i as u32, r));
                break;
            }
        }
        let (page, r) = match placed {
            Some(x) => x,
            None => {
                let mut p = MaxRects::new(opts.page_w, opts.page_h);
                let Some(r) = p.find(pw, ph) else {
                    return Err(Error2d::AtlasFull {
                        why: format!("image {key} does not fit an empty page"),
                    });
                };
                p.place(r);
                pages.push(p);
                ((pages.len() - 1) as u32, r)
            }
        };
        out.insert(
            key,
            Region {
                page,
                x: r.x + pad,
                y: r.y + pad,
                w,
                h,
            },
        );
    }
    Ok((out, pages.len() as u32))
}

/// The pixel size of an image file from its header alone (no decode): PNG (the IHDR
/// chunk) and Aseprite (the file header: the size of the sheet the importer lays its frames
/// out on, [`crate::aseprite::sheet_grid`] cells of the canvas size, so a slicing drawn in
/// the editor matches the texture the game gets); `None` for anything else or a short file.
#[must_use]
pub fn image_file_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let be = |i: usize| -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(i..i + 4)?.try_into().ok()?))
    };
    let le16 = |i: usize| -> Option<u32> {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(i..i + 2)?.try_into().ok()?,
        )))
    };
    if bytes.get(..8) == Some(&PNG[..]) && bytes.get(12..16) == Some(&b"IHDR"[..]) {
        let (w, h) = (be(16)?, be(20)?);
        return (w > 0 && h > 0).then_some((w, h));
    }
    if bytes.get(4..6) == Some(&0xA5E0u16.to_le_bytes()[..]) {
        let (w, h, frames) = (le16(8)?, le16(10)?, le16(6)?);
        let (cols, rows) = crate::aseprite::sheet_grid(frames);
        let (sw, sh) = (w.checked_mul(cols)?, h.checked_mul(rows)?);
        return (sw > 0 && sh > 0).then_some((sw, sh));
    }
    None
}

/// An RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    /// `w * h * 4` bytes, row-major, top row first.
    pub rgba: Vec<u8>,
}

impl Image {
    /// A `w x h` image filled with `rgba`.
    #[must_use]
    pub fn filled(w: u32, h: u32, rgba: [u8; 4]) -> Self {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&rgba);
        }
        Self { w, h, rgba: v }
    }

    /// The pixel at `(x, y)`.
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    pub fn set(&mut self, x: u32, y: u32, p: [u8; 4]) {
        let i = ((y * self.w + x) * 4) as usize;
        self.rgba[i..i + 4].copy_from_slice(&p);
    }

    fn check(&self) -> Result<(), Error2d> {
        if self.rgba.len() != (self.w as usize) * (self.h as usize) * 4 {
            return Err(Error2d::invalid(format!(
                "a {}x{} image needs {} bytes, has {}",
                self.w,
                self.h,
                self.w as usize * self.h as usize * 4,
                self.rgba.len()
            )));
        }
        Ok(())
    }
}

/// A packed atlas: pages of pixels and where each image went.
#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    pub opts: AtlasOptions,
    pub pages: Vec<Image>,
    /// A normal-map page per colour page, when any image had a normal map (flat
    /// `(128, 128, 255)` where an image had none).
    pub normal_pages: Vec<Image>,
    pub regions: BTreeMap<u32, Region>,
}

impl Atlas {
    /// Pack `images` (`(key, colour, optional normal map of the same size)`).
    pub fn build(
        images: &[(u32, &Image, Option<&Image>)],
        opts: AtlasOptions,
    ) -> Result<Atlas, Error2d> {
        for (k, img, n) in images {
            img.check()?;
            if let Some(n) = n {
                n.check()?;
                if (n.w, n.h) != (img.w, img.h) {
                    return Err(Error2d::invalid(format!(
                        "image {k}: its normal map is {}x{}, the image {}x{}",
                        n.w, n.h, img.w, img.h
                    )));
                }
            }
        }
        let sizes: Vec<(u32, u32, u32)> = images.iter().map(|(k, i, _)| (*k, i.w, i.h)).collect();
        let (regions, count) = pack_rects(&sizes, &opts)?;
        let blank = Image::filled(opts.page_w, opts.page_h, [0, 0, 0, 0]);
        let mut pages = vec![blank; count as usize];
        let any_normals = images.iter().any(|(_, _, n)| n.is_some());
        let mut normal_pages = if any_normals {
            vec![Image::filled(opts.page_w, opts.page_h, [128, 128, 255, 0]); count as usize]
        } else {
            Vec::new()
        };
        let e = opts.extrude.min(opts.padding);
        for (k, img, n) in images {
            let Some(r) = regions.get(k) else { continue };
            blit(&mut pages[r.page as usize], img, r, e);
            if any_normals {
                match n {
                    Some(n) => blit(&mut normal_pages[r.page as usize], n, r, e),
                    None => {
                        let flat = Image::filled(img.w, img.h, [128, 128, 255, 255]);
                        blit(&mut normal_pages[r.page as usize], &flat, r, e);
                    }
                }
            }
        }
        Ok(Atlas {
            opts,
            pages,
            normal_pages,
            regions,
        })
    }

    /// Filled fraction of all pages (image pixels over page pixels).
    #[must_use]
    pub fn occupancy(&self) -> f64 {
        let used: u64 = self
            .regions
            .values()
            .map(|r| u64::from(r.w) * u64::from(r.h))
            .sum();
        let total =
            self.pages.len() as u64 * u64::from(self.opts.page_w) * u64::from(self.opts.page_h);
        if total == 0 {
            0.0
        } else {
            used as f64 / total as f64
        }
    }
}

/// Copy `img` into `page` at `r`, extruding its border `e` pixels outward.
fn blit(page: &mut Image, img: &Image, r: &Region, e: u32) {
    let e = i64::from(e);
    for y in -e..i64::from(img.h) + e {
        for x in -e..i64::from(img.w) + e {
            let sx = x.clamp(0, i64::from(img.w) - 1) as u32;
            let sy = y.clamp(0, i64::from(img.h) - 1) as u32;
            let (dx, dy) = (i64::from(r.x) + x, i64::from(r.y) + y);
            if dx < 0 || dy < 0 || dx >= i64::from(page.w) || dy >= i64::from(page.h) {
                continue;
            }
            page.set(dx as u32, dy as u32, img.get(sx, sy));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_overlap(regions: &BTreeMap<u32, Region>, pad: u32) -> bool {
        let v: Vec<&Region> = regions.values().collect();
        for (i, a) in v.iter().enumerate() {
            for b in &v[i + 1..] {
                if a.page != b.page {
                    continue;
                }
                let ra = Rect {
                    x: a.x - pad,
                    y: a.y - pad,
                    w: a.w + 2 * pad,
                    h: a.h + 2 * pad,
                };
                let rb = Rect {
                    x: b.x - pad,
                    y: b.y - pad,
                    w: b.w + 2 * pad,
                    h: b.h + 2 * pad,
                };
                if ra.intersects(&rb) {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn packs_without_overlap_inside_pages_and_deterministically() {
        let s = crate::math::Stream::new(9);
        let sizes: Vec<(u32, u32, u32)> = (0..300)
            .map(|i| {
                (
                    i,
                    4 + (s.bits(2 * u64::from(i)) % 60) as u32,
                    4 + (s.bits(2 * u64::from(i) + 1) % 60) as u32,
                )
            })
            .collect();
        let opts = AtlasOptions {
            page_w: 512,
            page_h: 512,
            padding: 2,
            extrude: 1,
        };
        let (a, pages) = pack_rects(&sizes, &opts).expect("pack");
        let (b, _) = pack_rects(&sizes, &opts).expect("pack");
        assert_eq!(a, b, "deterministic");
        assert_eq!(a.len(), 300);
        assert!(no_overlap(&a, 2));
        for r in a.values() {
            assert!(r.x >= 2 && r.y >= 2 && r.x + r.w + 2 <= 512 && r.y + r.h + 2 <= 512);
        }
        let used: u64 = sizes
            .iter()
            .map(|s| u64::from(s.1 + 4) * u64::from(s.2 + 4))
            .sum();
        let eff = used as f64 / (f64::from(pages) * 512.0 * 512.0);
        assert!(
            eff > 0.7,
            "MaxRects fills pages well: {eff:.3} over {pages} pages"
        );
    }

    #[test]
    fn a_too_large_image_is_refused_with_its_code() {
        let e = pack_rects(
            &[(1, 600, 10)],
            &AtlasOptions {
                page_w: 512,
                page_h: 512,
                padding: 0,
                extrude: 0,
            },
        )
        .expect_err("too big");
        assert_eq!(e.code().as_str(), "TWOD-0004");
    }

    #[test]
    fn pixels_land_where_the_region_says_and_edges_extrude() {
        let mut img = Image::filled(3, 2, [10, 20, 30, 255]);
        img.set(0, 0, [255, 0, 0, 255]);
        let atlas = Atlas::build(
            &[(5, &img, None)],
            AtlasOptions {
                page_w: 16,
                page_h: 16,
                padding: 2,
                extrude: 1,
            },
        )
        .expect("atlas");
        let r = atlas.regions[&5];
        let page = &atlas.pages[0];
        assert_eq!(page.get(r.x, r.y), [255, 0, 0, 255]);
        assert_eq!(
            page.get(r.x - 1, r.y - 1),
            [255, 0, 0, 255],
            "the corner is extruded"
        );
        assert_eq!(
            page.get(r.x - 2, r.y - 2),
            [0, 0, 0, 0],
            "beyond the extrusion stays clear"
        );
        assert!(atlas.normal_pages.is_empty());
        assert!(atlas.occupancy() > 0.0);
    }
}

#[cfg(test)]
mod file_size_tests {
    use super::*;
    use crate::aseprite::{AseWrite, decode, to_sheet, write};

    #[test]
    fn an_aseprite_header_answers_the_sheet_the_importer_lays_out() {
        for frames in [1u16, 2, 3, 4, 5, 10] {
            let file = write(&AseWrite {
                w: 8,
                h: 6,
                layers: vec![("a".into(), true, 255)],
                frames: (0..frames)
                    .map(|_| (100, vec![(0, 0, 0, Image::filled(8, 6, [1, 2, 3, 255]))]))
                    .collect(),
                tags: Vec::new(),
                compress: false,
            });
            let (sheet, _, _) = to_sheet(&decode(&file).expect("decode"));
            assert_eq!(
                image_file_size(&file),
                Some((sheet.w, sheet.h)),
                "{frames} frames"
            );
        }
        assert_eq!(image_file_size(b"not an image"), None);
    }
}
