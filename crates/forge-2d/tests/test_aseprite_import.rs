//! Aseprite import end to end (Ch.35 §35.2): an `.aseprite` file written into a project goes
//! through the asset server with `forge.2d` installed (the path the editor takes) and comes
//! out as a texture artefact plus a `sprite_sheet` artefact that decode back to the file's
//! frames, the sheet's slicing and its tags.
//!
//! The pixel checks compare against the colours the cels were authored with, not against
//! forge-2d's own decoder, so a misreading of the format that decode and the importer share
//! still fails here.
//!
//! Control: a truncated file must fail the import with `TWOD-0003` through the server (the
//! error reaches the caller; no sprite sheet is stored), so the success path is not vacuous.

use std::time::Duration;

use forge_2d::Plugin2d;
use forge_2d::anim::{Playback, slice};
use forge_2d::aseprite::{AseTag, AseWrite, write};
use forge_2d::atlas::Image;
use forge_2d::plugin::SpriteSheetAsset;
use forge_asset::{AssetServer, FirstPartyAssets, Settings, TextureAsset};
use forge_plugin::{Extensions, Grants, loader};
use forge_store::{Bytes, MemoryStore, StorePath};

const WAIT: Duration = Duration::from_secs(20);

fn p(s: &str) -> StorePath {
    StorePath::new(s).expect("path")
}

/// The asset server with the first-party asset types and `forge.2d` loaded, as the editor
/// shell loads them.
fn server(files: &[(&str, Vec<u8>)]) -> AssetServer {
    let mut x = Extensions::new();
    AssetServer::define_points(&mut x).expect("points");
    let types = FirstPartyAssets::new().expect("types");
    let two_d = Plugin2d::new().expect("plugin");
    loader::load(&mut x, &[&types, &two_d], &[], &Grants::new()).expect("loads");
    let mut store = MemoryStore::new("ada");
    for (path, b) in files {
        forge_store::ProjectStore::write(&mut store, &p(path), Bytes::from(b.clone()))
            .expect("write");
    }
    AssetServer::open(Box::new(store), &x).expect("opens").0
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const SLATE: [u8; 4] = [10, 20, 30, 255];
const GREY: [u8; 4] = [9, 9, 9, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

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

/// An 8x6, three-frame file: a red/blue checker; an opaque slate frame with a white cel on a
/// hidden layer; a grey cel partly off the canvas. Two tags.
fn hero() -> Vec<u8> {
    write(&AseWrite {
        w: 8,
        h: 6,
        layers: vec![("body".into(), true, 255), ("hidden".into(), false, 255)],
        frames: vec![
            (100, vec![(0, 0, 0, checker(8, 6, RED, BLUE))]),
            (
                150,
                vec![
                    (0, 0, 0, Image::filled(8, 6, SLATE)),
                    (1, 0, 0, Image::filled(8, 6, [255; 4])),
                ],
            ),
            (50, vec![(0, -2, 3, Image::filled(4, 4, GREY))]),
        ],
        tags: vec![
            AseTag {
                name: "walk".into(),
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
        compress: true,
    })
}

/// The expected pixel of frame `f` at `(x, y)`, from the authored cels.
fn authored(f: u32, x: u32, y: u32) -> [u8; 4] {
    match f {
        0 => {
            if (x + y) % 2 == 1 {
                BLUE
            } else {
                RED
            }
        }
        1 => SLATE,
        // The 4x4 cel at (-2, 3): canvas columns 0..2, rows 3..6.
        _ => {
            if x < 2 && y >= 3 {
                GREY
            } else {
                CLEAR
            }
        }
    }
}

#[test]
fn an_aseprite_file_imports_to_a_texture_and_a_sprite_sheet() {
    let mut a = server(&[("art/hero.aseprite", hero())]);
    let mut ev = Vec::new();
    let id = a
        .import(&p("art/hero.aseprite"), Settings::new(), &mut ev)
        .unwrap_or_else(|e| panic!("import: {e}"));

    let sheet = a
        .load::<SpriteSheetAsset>(id)
        .wait(WAIT)
        .unwrap_or_else(|e| panic!("sprite sheet: {e}"));
    assert!(sheet.warnings.is_empty(), "{:?}", sheet.warnings);

    // The slicing: three 8x6 frames on a 2x2 grid.
    let rects = slice(&sheet.slice);
    assert_eq!((sheet.slice.cell_w, sheet.slice.cell_h), (8, 6));
    assert_eq!((sheet.slice.image_w, sheet.slice.image_h), (16, 12));
    assert!(rects.len() >= 3, "{rects:?}");

    // The texture the sheet names: sRGB, one level (pixel art: no mips by default), and
    // each frame rect holds the authored pixels.
    let tex = a
        .load::<TextureAsset>(id.child(&sheet.texture))
        .wait(WAIT)
        .unwrap_or_else(|e| panic!("texture: {e}"));
    assert_eq!((tex.width, tex.height, tex.srgb), (16, 12, true));
    assert_eq!(tex.levels.len(), 1);
    let level = &tex.levels[0];
    for (f, r) in rects.iter().take(3).enumerate() {
        for y in 0..r.h {
            for x in 0..r.w {
                let o = (((r.y + y) * tex.width + r.x + x) * 4) as usize;
                let got = [level[o], level[o + 1], level[o + 2], level[o + 3]];
                assert_eq!(
                    got,
                    authored(f as u32, x, y),
                    "frame {f} pixel ({x}, {y}): the hidden layer never shows, off-canvas \
                     pixels are clipped"
                );
            }
        }
    }

    // The tags, as animations with the per-frame durations.
    let names: Vec<&str> = sheet.anims.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["walk", "bob"]);
    let walk = &sheet.anims[0];
    assert_eq!(walk.frames, [0, 1]);
    assert_eq!(walk.durations_us, [100_000, 150_000]);
    assert_eq!(walk.playback, Playback::Loop);
    let bob = &sheet.anims[1];
    assert_eq!(bob.frames, [0, 1, 2]);
    assert_eq!(bob.durations_us, [100_000, 150_000, 50_000]);
    assert_eq!(bob.playback, Playback::PingPong);
}

#[test]
fn control_a_truncated_file_fails_the_import_visibly() {
    let good = hero();
    let mut a = server(&[("art/bad.ase", good[..good.len() / 2].to_vec())]);
    let mut ev = Vec::new();
    let e = a
        .import(&p("art/bad.ase"), Settings::new(), &mut ev)
        .expect_err("a truncated file must not import");
    let text = e.to_string();
    assert!(text.contains("TWOD-0003"), "{text}");
    if let Some(id) = a.id_of(&p("art/bad.ase")) {
        assert!(
            a.load::<SpriteSheetAsset>(id).wait(WAIT).is_err(),
            "no sprite sheet is stored for a failed import"
        );
    }
}
