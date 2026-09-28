//! Reference content and scenes, generated from code (no asset files): a blob-47 tile set,
//! a character, a coin, bricks with a normal map, a background strip. The golden images
//! (`tests/test_2d_goldens.rs`), the perf gate's 2D rows and the sample game build on them,
//! so every one of them draws the same bytes on every machine.

use std::collections::BTreeMap;

use forge_frames::{FrameId, FramePos2};
use forge_num::DVec2;

use crate::Error2d;
use crate::anim::{FrameAnim, Playback};
use crate::atlas::{Atlas, AtlasOptions, Image};
use crate::camera::Camera2d;
use crate::light::{Light2d, LightShape, Occluder};
use crate::math::Stream;
use crate::parallax::ParallaxLayer;
use crate::particles::{Emitter, ParticleSystem};
use crate::skeleton::{Bone2d, BoneLocal, Clip, Part, Skeleton2d, Track};
use crate::spline::SplineShape;
use crate::sprite::{Frame2d, Sprite, TextureId, UvRect};
use crate::tilemap::{Cell, E, N, NE, NW, S, SE, SW, Tilemap, Tileset, W, blob47};

/// Tile size of the generated tile set (pixels).
pub const TILE_PX: u32 = 16;

fn rgb(r: u8, g: u8, b: u8) -> [u8; 4] {
    [r, g, b, 255]
}

fn shade(c: [u8; 4], k: i32) -> [u8; 4] {
    let f = |v: u8| (i32::from(v) + k).clamp(0, 255) as u8;
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

/// The blob-47 tile set image: 8 columns x 6 rows of 16 px tiles, tile `i` drawn for the
/// `i`-th canonical mask (dirt, grass where the north edge is open, dark rims on open
/// edges, notched inner corners), plus tile 47: a stone block.
#[must_use]
pub fn tileset_image() -> Image {
    let cols = 8;
    let rows = 6;
    let mut img = Image::filled(cols * TILE_PX, rows * TILE_PX, [0, 0, 0, 0]);
    let noise = Stream::new(0x0071_15E7);
    let masks = blob47();
    for (i, m) in masks.iter().copied().enumerate().chain([(47usize, 0xFFu8)]) {
        let (tx, ty) = ((i as u32 % cols) * TILE_PX, (i as u32 / cols) * TILE_PX);
        let stone = i == 47;
        for y in 0..TILE_PX {
            for x in 0..TILE_PX {
                let n = (noise.bits(u64::from((tx + x) * 977 + (ty + y) * 131)) % 24) as i32 - 12;
                let mut c = if stone {
                    shade(rgb(120, 120, 130), n)
                } else {
                    shade(rgb(134, 92, 58), n)
                };
                let edge = 2;
                let open = |bit: u8| m & bit == 0;
                if !stone {
                    if open(N) && y < 4 {
                        c = shade(rgb(70, 160, 60), n);
                    }
                    if (open(N) && y < edge)
                        || (open(S) && y >= TILE_PX - edge)
                        || (open(W) && x < edge)
                        || (open(E) && x >= TILE_PX - edge)
                    {
                        c = shade(c, -45);
                    }
                    // Inner corners: both edges closed, the corner open.
                    let corner = |bit: u8, a: u8, b: u8, cx: bool, cy: bool| {
                        m & bit == 0 && m & a != 0 && m & b != 0 && cx && cy
                    };
                    if corner(NE, N, E, x >= TILE_PX - edge, y < edge)
                        || corner(NW, N, W, x < edge, y < edge)
                        || corner(SE, S, E, x >= TILE_PX - edge, y >= TILE_PX - edge)
                        || corner(SW, S, W, x < edge, y >= TILE_PX - edge)
                    {
                        c = shade(c, -45);
                    }
                } else if x == 0 || y == 0 || x == TILE_PX - 1 || y == TILE_PX - 1 {
                    c = shade(c, -40);
                }
                img.set(tx + x, ty + y, c);
            }
        }
    }
    img
}

/// The generated tile set over `texture` (the image of [`tileset_image`]): terrain 0 is the
/// blob, tile 47 a solid stone block; every tile is solid.
#[must_use]
pub fn tileset(texture: TextureId) -> Tileset {
    let mut rules = BTreeMap::new();
    for (i, m) in blob47().into_iter().enumerate() {
        rules.insert(m, i as u32);
    }
    Tileset {
        texture,
        texture_size: (8 * TILE_PX, 6 * TILE_PX),
        origin_px: (0, 0),
        tile_px: (TILE_PX, TILE_PX),
        columns: 8,
        count: 48,
        terrains: vec![rules],
        solid: (0..48).collect(),
    }
}

/// A 16x16 character (4 frames of a walk cycle across, 64x16).
#[must_use]
pub fn hero_image() -> Image {
    let mut img = Image::filled(64, 16, [0, 0, 0, 0]);
    for f in 0..4u32 {
        let ox = f * 16;
        for y in 0..16u32 {
            for x in 0..16u32 {
                let (cx, cy) = (x as i32 - 8, y as i32);
                let c = if (2..7).contains(&cy) && cx.abs() <= 3 {
                    Some(rgb(240, 200, 160)) // head
                } else if (7..12).contains(&cy) && cx.abs() <= 3 {
                    Some(rgb(200, 40, 50)) // body
                } else if (12..16).contains(&cy) {
                    let swing = [0, 1, 0, -1][f as usize];
                    let left = cx == -2 + swing || cx == -1 + swing;
                    let right = cx == 1 - swing || cx == 2 - swing;
                    (left || right).then(|| rgb(40, 50, 120)) // legs
                } else {
                    None
                };
                if let Some(c) = c {
                    img.set(ox + x, y, c);
                }
            }
        }
        img.set(ox + 9, 4, rgb(20, 20, 20));
    }
    img
}

/// The hero's walk animation (frames 0-3 of [`hero_image`] at 8 fps).
pub fn hero_walk() -> Result<FrameAnim, Error2d> {
    FrameAnim::at_fps("walk", vec![0, 1, 2, 3], 8.0, Playback::Loop)
}

/// A 12x12 coin.
#[must_use]
pub fn coin_image() -> Image {
    let mut img = Image::filled(12, 12, [0, 0, 0, 0]);
    for y in 0..12 {
        for x in 0..12 {
            let (dx, dy) = (f64::from(x) - 5.5, f64::from(y) - 5.5);
            let d2 = dx * dx + dy * dy;
            if d2 <= 30.0 {
                let c = if d2 > 20.0 {
                    rgb(180, 130, 20)
                } else if dx < -1.0 && dy < -1.0 {
                    rgb(255, 240, 150)
                } else {
                    rgb(240, 190, 40)
                };
                img.set(x, y, c);
            }
        }
    }
    img
}

/// A 64x64 brick wall and its normal map (bevelled mortar lines).
#[must_use]
pub fn bricks() -> (Image, Image) {
    let (w, h) = (64u32, 64u32);
    let mut c = Image::filled(w, h, [0, 0, 0, 255]);
    let mut n = Image::filled(w, h, [128, 128, 255, 255]);
    let noise = Stream::new(0xB21C);
    for y in 0..h {
        let row = y / 16;
        let off = if row % 2 == 0 { 0 } else { 16 };
        for x in 0..w {
            let bx = (x + off) % 32;
            let by = y % 16;
            let mortar = bx < 2 || by < 2;
            let k = (noise.bits(u64::from(y * w + x)) % 20) as i32 - 10;
            c.set(
                x,
                y,
                if mortar {
                    shade(rgb(90, 88, 84), k)
                } else {
                    shade(rgb(168, 72, 52), k)
                },
            );
            // Normals: bricks bevel toward the mortar on their first/last 3 px.
            let (mut nx, mut ny) = (0i32, 0i32);
            if !mortar {
                if bx < 5 {
                    nx = -90;
                } else if bx > 28 {
                    nx = 90;
                }
                if by < 5 {
                    ny = 90;
                } else if by > 12 {
                    ny = -90;
                }
            }
            let enc = |v: i32| (128 + v).clamp(0, 255) as u8;
            let nz = if nx != 0 || ny != 0 { 200 } else { 255 };
            n.set(x, y, [enc(nx), enc(ny), nz, 255]);
        }
    }
    (c, n)
}

/// A 32x16 background strip (hills), for a repeating parallax layer.
#[must_use]
pub fn hills_image() -> Image {
    let mut img = Image::filled(64, 32, [0, 0, 0, 0]);
    for x in 0..64u32 {
        let t = f64::from(x) / 64.0 * core::f64::consts::TAU;
        let top = 14.0 + 6.0 * forge_num::det::sin(t) + 3.0 * forge_num::det::sin(3.0 * t + 1.0);
        for y in 0..32u32 {
            if f64::from(y) >= top {
                img.set(x, y, rgb(70, 100, 140));
            }
        }
    }
    img
}

/// Keys of the generated sprite atlas.
pub const ATLAS_HERO: u32 = 1;
pub const ATLAS_COIN: u32 = 2;
pub const ATLAS_HILLS: u32 = 3;

/// The hero, the coin and the hills packed into one 128x128 atlas page.
pub fn sprite_atlas() -> Result<Atlas, Error2d> {
    let (hero, coin, hills) = (hero_image(), coin_image(), hills_image());
    Atlas::build(
        &[
            (ATLAS_HERO, &hero, None),
            (ATLAS_COIN, &coin, None),
            (ATLAS_HILLS, &hills, None),
        ],
        AtlasOptions {
            page_w: 128,
            page_h: 128,
            padding: 1,
            extrude: 1,
        },
    )
}

/// The texture ids a scene draws with.
#[derive(Clone, Copy, Debug)]
pub struct SceneTextures {
    pub tiles: TextureId,
    pub atlas: TextureId,
    pub bricks: TextureId,
    pub white: TextureId,
}

fn at(x: f64, y: f64) -> FramePos2 {
    FramePos2::new(FrameId(0), DVec2::new(x, y))
}

/// A level strip: ground terrain from x = -40 to 80 at row 8 and below, a few platforms,
/// stone blocks. Cell size 1 unit; the map's origin is `(0, 4)` (row 0 at y = 4).
pub fn level(tiles: TextureId) -> Result<Tilemap, Error2d> {
    let mut m = Tilemap::new(at(0.0, 4.0), DVec2::new(1.0, 1.0), tileset(tiles));
    let l = m.add_layer("ground", 0, true);
    for x in -40i32..80 {
        let depth = 8 + (x.rem_euclid(13) / 7);
        for y in depth..14 {
            m.set(l, x, y, Cell::Terrain(0))?;
        }
    }
    for (x0, x1, y) in [(3, 8, 5), (12, 15, 3), (20, 26, 6), (32, 36, 4)] {
        for x in x0..x1 {
            m.set(l, x, y, Cell::Terrain(0))?;
        }
    }
    for (x, y) in [(10, 7), (10, 6), (28, 7), (29, 7), (29, 6)] {
        m.set(l, x, y, Cell::Tile(47))?;
    }
    Ok(m)
}

/// Golden scene 1: autotiled terrain, atlas sprites (flipped, rotated, tinted), a
/// repeating parallax background, pixel-perfect at 320x180.
pub fn tiles_scene(t: &SceneTextures, atlas: &Atlas) -> Result<Frame2d, Error2d> {
    let cam = Camera2d::pixel_perfect(at(6.3, 0.2), (320, 180), 16.0);
    let mut f = Frame2d::new(cam);
    f.clear = [0.45, 0.7, 0.95, 1.0];
    let map = level(t.tiles)?;
    let view = crate::math::Aabb2::new(DVec2::new(-20.0, -20.0), DVec2::new(30.0, 20.0));
    map.sprites(&view, &mut f.sprites);
    let uv = |k: u32, fx: u32, w: u32, h: u32| {
        let r = atlas.regions[&k];
        UvRect::from_pixels(
            f64::from(r.x + fx),
            f64::from(r.y),
            f64::from(w),
            f64::from(h),
            atlas.opts.page_w,
            atlas.opts.page_h,
        )
    };
    // Hills: parallax 0.3, repeating every 4 units.
    f.parallax
        .insert(-10, ParallaxLayer::new(0.3, 0.3).repeating(4.0, 0.0));
    let mut hills = Sprite::new(at(0.0, -1.0), DVec2::new(4.0, 2.0), t.atlas).at_layer(-10, 0);
    hills.uv = uv(ATLAS_HILLS, 0, 64, 32);
    f.sprites.push(hills);
    for (i, x) in [2.0, 4.0, 13.0].into_iter().enumerate() {
        let mut hero = Sprite::new(at(x, -3.5), DVec2::new(1.0, 1.0), t.atlas).at_layer(5, 0);
        hero.uv = uv(ATLAS_HERO, 16 * i as u32, 16, 16);
        hero.flip_x = i == 1;
        f.sprites.push(hero);
    }
    for i in 0..6 {
        let mut coin = Sprite::new(
            at(5.0 + f64::from(i) * 0.9, 1.7),
            DVec2::new(0.75, 0.75),
            t.atlas,
        )
        .at_layer(4, 0);
        coin.uv = uv(ATLAS_COIN, 0, 12, 12);
        coin.angle = f64::from(i) * 0.3;
        if i == 5 {
            coin.tint = [0.4, 1.0, 0.4, 1.0];
        }
        f.sprites.push(coin);
    }
    Ok(f)
}

/// Golden scene 2: a normal-mapped brick wall lit by a point light, a spot light and a
/// coloured light in the dark, with pillars and a crate casting hard shadows. 64 px a unit:
/// a 64-texel brick sprite two units wide is exactly 2 px a texel, so no pixel centre falls
/// on a texel edge — nearest sampling then gives the same texel on every rasteriser (at
/// 80 px a unit, 2.5 px a texel, lavapipe and the RTX 3080 rounded the edges differently).
pub fn lights_scene(t: &SceneTextures) -> Result<Frame2d, Error2d> {
    let cam = Camera2d::smooth(at(0.0, 0.0), 64.0);
    let mut f = Frame2d::new(cam);
    f.ambient = [0.06, 0.06, 0.08];
    f.clear = [0.0, 0.0, 0.0, 1.0];
    for y in -2..2 {
        for x in -4..4 {
            let s = Sprite::new(
                at(f64::from(x) * 2.0 + 1.0, f64::from(y) * 2.0 + 1.0),
                DVec2::new(2.0, 2.0),
                t.bricks,
            )
            .at_layer(0, 0);
            f.sprites.push(s);
        }
    }
    for (x, y, w, h) in [
        (-3.0, -2.5, 0.5, 2.0),
        (2.0, -2.5, 0.5, 2.5),
        (-0.5, 0.5, 1.0, 0.6),
    ] {
        let mut s =
            Sprite::new(at(x + w * 0.5, y + h * 0.5), DVec2::new(w, h), t.white).at_layer(1, 0);
        s.tint = [0.1, 0.1, 0.12, 1.0];
        f.sprites.push(s);
        f.occluders.push(Occluder::rect(
            FrameId(0),
            DVec2::new(x, y),
            DVec2::new(x + w, y + h),
        ));
    }
    let mut warm = Light2d::point(at(-1.5, 1.5), 6.0, 2.2);
    warm.color = [1.0, 0.8, 0.55];
    warm.height = 0.8;
    f.lights.push(warm);
    let mut spot = Light2d::point(at(4.5, 2.8), 9.0, 3.0);
    spot.shape = LightShape::Spot {
        direction: -2.3,
        inner: 0.25,
        outer: 0.45,
    };
    spot.height = 1.2;
    f.lights.push(spot);
    let mut cold = Light2d::point(at(0.5, -2.2), 4.0, 1.6);
    cold.color = [0.3, 0.6, 1.0];
    cold.height = 0.5;
    f.lights.push(cold);
    Ok(f)
}

/// A two-bone-per-leg cutout figure: hip root, torso, head, two legs of thigh + shin.
#[must_use]
pub fn cutout() -> (Skeleton2d, Vec<Part>, Clip) {
    let b = |name: &str, parent: Option<usize>, x: f64, y: f64, angle: f64, len: f64| Bone2d {
        name: name.into(),
        parent,
        rest: BoneLocal {
            offset: DVec2::new(x, y),
            angle,
            scale: DVec2::new(1.0, 1.0),
        },
        length: len,
    };
    let down = -core::f64::consts::FRAC_PI_2;
    let sk = Skeleton2d {
        bones: vec![
            b("hip", None, 0.0, 0.0, 0.0, 0.0),
            b(
                "torso",
                Some(0),
                0.0,
                0.0,
                core::f64::consts::FRAC_PI_2,
                0.8,
            ),
            b("head", Some(1), 0.8, 0.0, 0.0, 0.4),
            b("thigh_l", Some(0), -0.15, 0.0, down, 0.5),
            b("shin_l", Some(3), 0.5, 0.0, 0.0, 0.5),
            b("thigh_r", Some(0), 0.15, 0.0, down, 0.5),
            b("shin_r", Some(5), 0.5, 0.0, 0.0, 0.5),
        ],
    };
    let part = |bone: usize, w: f64, h: f64, z: i32| Part {
        bone,
        size: DVec2::new(w, h),
        pivot: DVec2::new(0.0, 0.5),
        offset: DVec2::ZERO,
        angle: 0.0,
        uv: UvRect::FULL,
        texture: TextureId(0),
        z,
    };
    let parts = vec![
        part(1, 0.8, 0.45, 1),
        part(2, 0.45, 0.4, 2),
        part(3, 0.5, 0.18, 0),
        part(4, 0.5, 0.16, 0),
        part(5, 0.5, 0.18, 3),
        part(6, 0.5, 0.16, 3),
    ];
    let swing = 0.5;
    let clip = Clip {
        name: "walk".into(),
        duration_us: 800_000,
        looping: true,
        tracks: vec![
            Track {
                bone: 3,
                rotation: vec![(0, swing), (400_000, -swing), (800_000, swing)],
                ..Track::default()
            },
            Track {
                bone: 5,
                rotation: vec![(0, -swing), (400_000, swing), (800_000, -swing)],
                ..Track::default()
            },
            Track {
                bone: 4,
                rotation: vec![
                    (0, 0.0),
                    (200_000, 0.6),
                    (400_000, 0.0),
                    (600_000, 0.1),
                    (800_000, 0.0),
                ],
                ..Track::default()
            },
            Track {
                bone: 6,
                rotation: vec![
                    (0, 0.1),
                    (200_000, 0.0),
                    (400_000, 0.0),
                    (600_000, 0.6),
                    (800_000, 0.1),
                ],
                ..Track::default()
            },
            Track {
                bone: 0,
                translation: vec![
                    (0, DVec2::ZERO),
                    (200_000, DVec2::new(0.0, 0.05)),
                    (400_000, DVec2::ZERO),
                    (600_000, DVec2::new(0.0, 0.05)),
                    (800_000, DVec2::ZERO),
                ],
                ..Track::default()
            },
        ],
    };
    (sk, parts, clip)
}

/// Golden scene 3: a spline hill (fill and grass edge), a particle fountain after 90 fixed
/// steps, and the cutout figure posed 0.3 s into its walk.
pub fn shapes_scene(t: &SceneTextures) -> Result<Frame2d, Error2d> {
    let cam = Camera2d::smooth(at(0.0, -1.0), 64.0);
    let mut f = Frame2d::new(cam);
    f.clear = [0.12, 0.12, 0.16, 1.0];
    let mut hill = SplineShape::closed(
        at(0.0, 0.0),
        vec![
            DVec2::new(-9.0, -5.0),
            DVec2::new(9.0, -5.0),
            DVec2::new(9.0, -1.0),
            DVec2::new(4.0, 0.2),
            DVec2::new(0.0, -0.8),
            DVec2::new(-4.0, 0.6),
            DVec2::new(-9.0, -1.0),
        ],
        t.white,
        t.white,
    );
    hill.fill_color = [0.35, 0.25, 0.15, 1.0];
    hill.edge_color = [0.3, 0.7, 0.25, 1.0];
    hill.edge_width = 0.3;
    f.meshes.push(hill.fill()?);
    f.meshes.push(hill.edge()?);
    let mut fountain = ParticleSystem::new(
        Emitter {
            rate: 120.0,
            speed: (3.0, 4.5),
            spread: 0.3,
            gravity: DVec2::new(0.0, -6.0),
            size: (0.18, 0.05),
            color: ([0.6, 0.85, 1.0, 1.0], [0.2, 0.4, 1.0, 0.0]),
            ..Emitter::default()
        },
        at(4.0, 0.3),
        0xF0_0F,
    )?;
    for _ in 0..90 {
        fountain.step(1.0 / 60.0);
    }
    fountain.sprites(t.white, UvRect::FULL, 3, 0, &mut f.sprites);
    let (sk, mut parts, clip) = cutout();
    for p in &mut parts {
        p.texture = t.white;
    }
    let pose = sk.sample(&clip, 300_000);
    let root = crate::math::Xform2 {
        translation: DVec2::new(-3.0, 0.9),
        ..crate::math::Xform2::IDENTITY
    };
    let world = sk.world(&pose, &root);
    let mut figure = Skeleton2d::sprites(&parts, &world, at(0.0, 0.0), 2, 0);
    let colors = [
        [0.85, 0.2, 0.25, 1.0],
        [0.95, 0.8, 0.65, 1.0],
        [0.2, 0.25, 0.6, 1.0],
        [0.15, 0.2, 0.5, 1.0],
        [0.2, 0.25, 0.6, 1.0],
        [0.15, 0.2, 0.5, 1.0],
    ];
    for (s, c) in figure.iter_mut().zip(colors) {
        s.tint = c;
    }
    f.sprites.extend(figure);
    Ok(f)
}

/// The perf gate's 2D frame, drawn at its output size (a smooth camera, 40 px a unit: at
/// 1920x1080 the view is 48 x 27 units): a stone backdrop and a sparse front layer over the
/// level's terrain (about 2,700 tiles in view), 600 atlas sprites, 2,000 particles, a spline
/// shape, and 32 shadowed lights over the terrain's occluders.
pub fn perf_scene(t: &SceneTextures, atlas: &Atlas) -> Result<Frame2d, Error2d> {
    let cam = Camera2d::smooth(at(10.0, 0.0), 40.0);
    let mut f = Frame2d::new(cam);
    f.ambient = [0.25, 0.25, 0.3];
    let mut map = level(t.tiles)?;
    let back = map.add_layer("back", -1, false);
    let front = map.add_layer("front", 6, false);
    for x in -15..35 {
        for y in -10..19 {
            map.set(back, x, y, Cell::Tile(47))?;
            if (x + y) % 5 == 0 {
                map.set(front, x, y, Cell::Terrain(0))?;
            }
        }
    }
    let view = crate::math::Aabb2::new(DVec2::new(-15.0, -14.0), DVec2::new(35.0, 14.0));
    map.sprites(&view, &mut f.sprites);
    for b in map.solid_boxes() {
        f.occluders.push(Occluder::rect(
            FrameId(0),
            b.min + map.origin.local,
            b.max + map.origin.local,
        ));
    }
    let r = atlas.regions[&ATLAS_COIN];
    let coin_uv = UvRect::from_region(&r, atlas.opts.page_w, atlas.opts.page_h);
    let s = Stream::new(0x5EED);
    for i in 0..600u64 {
        let mut c = Sprite::new(
            at(s.range(2 * i, -12.0, 32.0), s.range(2 * i + 1, -12.0, 12.0)),
            DVec2::new(0.6, 0.6),
            t.atlas,
        )
        .at_layer(4, 0);
        c.uv = coin_uv;
        c.angle = s.range(i + 5000, 0.0, 6.0);
        f.sprites.push(c);
    }
    let mut ps = ParticleSystem::new(
        Emitter {
            rate: 2000.0,
            life: (0.9, 1.1),
            max: 2000,
            ..Emitter::default()
        },
        at(10.0, -2.0),
        7,
    )?;
    for _ in 0..60 {
        ps.step(1.0 / 60.0);
    }
    ps.sprites(t.white, UvRect::FULL, 5, 0, &mut f.sprites);
    let hill = SplineShape::closed(
        at(10.0, -4.0),
        vec![
            DVec2::new(-10.0, -2.0),
            DVec2::new(10.0, -2.0),
            DVec2::new(6.0, 1.0),
            DVec2::new(-6.0, 1.5),
        ],
        t.white,
        t.white,
    );
    f.meshes.push(hill.fill()?);
    f.meshes.push(hill.edge()?);
    for i in 0..32u64 {
        let mut l = Light2d::point(
            at(
                s.range(9000 + i, -10.0, 30.0),
                s.range(9100 + i, -10.0, 10.0),
            ),
            6.0,
            1.5,
        );
        l.color = [
            s.range(9200 + i, 0.3, 1.0),
            s.range(9300 + i, 0.3, 1.0),
            s.range(9400 + i, 0.3, 1.0),
        ];
        f.lights.push(l);
    }
    Ok(f)
}

/// Upload the generated textures (tile set, sprite atlas, bricks with their normal map) to
/// `r`: what the golden scenes, the perf frame and the sample game draw with.
#[cfg(feature = "render")]
pub fn upload(
    r: &mut crate::render::Renderer2d,
    dev: &forge_gpu::GpuDevice,
) -> Result<(SceneTextures, Atlas), Error2d> {
    use crate::render::Filter2d;
    let tiles = r.add_texture(dev, &tileset_image(), None, Filter2d::Nearest)?;
    let atlas = sprite_atlas()?;
    let page = atlas
        .pages
        .first()
        .ok_or_else(|| Error2d::invalid("the sprite atlas has no page"))?;
    let atlas_tex = r.add_texture(dev, page, None, Filter2d::Nearest)?;
    let (bc, bn) = bricks();
    let bricks = r.add_texture(dev, &bc, Some(&bn), Filter2d::Nearest)?;
    Ok((
        SceneTextures {
            tiles,
            atlas: atlas_tex,
            bricks,
            white: r.white(),
        },
        atlas,
    ))
}

/// A level's physics: the solid cells of [`level`] as merged static boxes.
pub fn level_physics(map: &Tilemap) -> Result<crate::physics::PhysicsWorld2d, Error2d> {
    use crate::physics::{BodyDef, BodyKind, ColliderDef, PhysicsWorld2d, Shape};
    let mut w = PhysicsWorld2d::new(map.frame());
    let ground = w.add_body(&BodyDef::new(BodyKind::Static, map.origin))?;
    for b in map.solid_boxes() {
        let mut c = ColliderDef::new(Shape::Box {
            half: b.size() * 0.5,
        });
        c.offset = b.center();
        w.add_collider(ground, c)?;
    }
    Ok(w)
}

/// The 2D cold start the perf gate budgets (`2d.cold_start`, Ch.31 §31.5): everything a
/// 2D level needs before its first frame that is not GPU work — the tile map built and
/// autotiled, its colliders merged into a physics world, the sprite atlas packed, the tile
/// set and the character drawn.
pub fn cold_start(
    tiles: TextureId,
) -> Result<(Tilemap, crate::physics::PhysicsWorld2d, Atlas), Error2d> {
    let map = level(tiles)?;
    let world = level_physics(&map)?;
    let atlas = sprite_atlas()?;
    let _ = tileset_image();
    Ok((map, world, atlas))
}

/// The perf gate's physics load (`phys2d.step`): the level's terrain and `n` dynamic bodies
/// (boxes, circles, capsules) dropped onto it and settled for 120 steps.
pub fn physics_pile(n: u32) -> Result<crate::physics::PhysicsWorld2d, Error2d> {
    use crate::physics::{BodyDef, BodyKind, ColliderDef, Shape};
    let map = level(TextureId(0))?;
    let mut w = level_physics(&map)?;
    for i in 0..n {
        let x = -30.0 + f64::from(i % 50) * 1.1;
        let y = 2.0 + f64::from(i / 50) * 1.1;
        let b = w.add_body(&BodyDef::new(BodyKind::Dynamic, at(x, y)))?;
        let shape = match i % 3 {
            0 => Shape::Box {
                half: DVec2::new(0.4, 0.4),
            },
            1 => Shape::Circle { radius: 0.4 },
            _ => Shape::Capsule {
                half_height: 0.2,
                radius: 0.3,
            },
        };
        w.add_collider(b, ColliderDef::new(shape))?;
    }
    for _ in 0..120 {
        w.step();
    }
    Ok(w)
}
