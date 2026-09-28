//! I2 — generation is a pure function of `(seed, integer position)` and is byte-identical on
//! every supported platform (Ch.3.4).
//!
//! A fixed corpus is generated, each row hashed with BLAKE3, and compared against
//! `tests/determinism/golden.txt`, committed. CI runs this on windows-x86_64 and
//! ubuntu-x86_64 (macOS is out of scope, E-33); the same golden file must pass on both.
//!
//! The corpus is what exists today (Ch.3.4 item 1, limited per WP-01): sweeps of every
//! `forge_num::det` function over normal, huge, tiny, subnormal and special inputs; Newton
//! solves through `newton::<5>`; the integer noise basis; 256 noise "chunks" (8 seeds x 4 LODs
//! x 8 chunks of 8^3 fBm samples); and, since WP-U15, the 2D pipeline (`2d/*`: physics,
//! autotiling, atlas packing, lighting geometry, rigs, particles, navigation, Aseprite, and the
//! sample game's scripted run). Appending rows never changes existing ones.
//!
//! **A golden change requires a plan amendment and a one-line reason in `docs/adr/`.**
//! There is deliberately no "bless" switch: on a mismatch the test prints the complete new
//! file, and a human decides whether to commit it.
//!
//! Positive control (W2): `positive_control_mutate_det_build_fails` builds this very test
//! with `--features mutate-det` (octave 0 of `fbm_i` perturbed by 1 ulp) and asserts that it
//! FAILS, on exactly the rows that depend on that octave.

use std::collections::BTreeMap;
use std::path::PathBuf;

use forge_num::{IVec3, det, fbm_i, hash3, newton, simplex_i, tree_sum, value_noise_i};

const GOLDEN: &str = "determinism/golden.txt";
const MISMATCH: &str = "GOLDEN MISMATCH";

/// Deterministic input stream (SplitMix64) — the corpus must not depend on anything but code.
struct Stream(u64);
impl Stream {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [a, b) from integer bits (no transcendental involved).
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }
    /// Any f64 bit pattern at all: NaNs, infinities, subnormals, huge, tiny.
    fn any(&mut self) -> f64 {
        f64::from_bits(self.next())
    }
    /// Log-uniform magnitude, random sign, binary exponent in [lo, hi].
    fn log_uniform(&mut self, lo: i32, hi: i32) -> f64 {
        let e = lo + (self.next() % (hi - lo + 1) as u64) as i32;
        let m = self.next();
        let bits = ((e + 1023) as u64) << 52 | (m & ((1 << 52) - 1)) | (m & (1 << 63));
        f64::from_bits(bits)
    }
}

const SPECIALS: [f64; 14] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    2.0,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    f64::MIN_POSITIVE,
    f64::MAX,
    f64::EPSILON,
    5e-324,
    1e22,
];

/// Hashes a row: every value's little-endian bytes, in order.
struct Row(blake3::Hasher);
impl Row {
    fn new() -> Self {
        Self(blake3::Hasher::new())
    }
    fn f(&mut self, v: f64) {
        self.0.update(&v.to_le_bytes());
    }
    fn u(&mut self, v: u64) {
        self.0.update(&v.to_le_bytes());
    }
    fn hex(&self) -> String {
        self.0.finalize().to_hex().to_string()
    }
}

/// One-argument inputs: specials, raw bit patterns, and each numerically interesting range.
fn unary_inputs(seed: u64) -> Vec<f64> {
    let mut s = Stream(seed);
    let mut v: Vec<f64> = SPECIALS.to_vec();
    for _ in 0..1024 {
        v.push(s.any());
    }
    for _ in 0..1024 {
        v.push(s.uniform(-4.0, 4.0));
    }
    for _ in 0..1024 {
        v.push(s.log_uniform(-1074 + 52, 1023));
    }
    for _ in 0..512 {
        v.push(s.uniform(-745.0, 710.0));
    }
    for _ in 0..512 {
        v.push(s.log_uniform(-60, 60));
    }
    v
}

fn corpus() -> BTreeMap<String, String> {
    let mut rows = BTreeMap::new();
    let mut put = |name: String, row: Row| {
        rows.insert(name, row.hex());
    };

    // ---- det transcendentals ----
    type F1 = fn(f64) -> f64;
    let unary: [(&str, F1); 6] = [
        ("sin", det::sin),
        ("cos", det::cos),
        ("exp", det::exp),
        ("ln", det::ln),
        ("sqrt", det::sqrt),
        ("ln_abs", |x| det::ln(x.abs())),
    ];
    for (i, (name, f)) in unary.iter().enumerate() {
        let mut r = Row::new();
        for x in unary_inputs(0xD37_0000 + i as u64) {
            r.f(f(x));
        }
        put(format!("det/{name}"), r);
    }
    {
        let mut s = Stream(0xD37_1000);
        let mut r = Row::new();
        for &x in &SPECIALS {
            for &y in &SPECIALS {
                r.f(det::pow(x, y));
                r.f(det::atan2(x, y));
            }
        }
        put("det/pow_atan2_specials".into(), r);
        let mut r = Row::new();
        for _ in 0..4096 {
            let x = s.log_uniform(-30, 30).abs();
            let y = s.uniform(-40.0, 40.0);
            r.f(det::pow(x, y));
            r.f(det::pow(-x, y.round()));
        }
        put("det/pow".into(), r);
        let mut r = Row::new();
        for _ in 0..4096 {
            r.f(det::atan2(s.log_uniform(-80, 80), s.log_uniform(-80, 80)));
            r.f(det::atan2(s.any(), s.any()));
        }
        put("det/atan2".into(), r);
    }

    // ---- newton: E - e sin E = M, exactly five steps (the Foundations' rule) ----
    {
        let mut s = Stream(0xD37_2000);
        let mut r = Row::new();
        for _ in 0..2048 {
            let e = s.uniform(0.0, 0.95);
            let m = s.uniform(-std::f64::consts::PI, std::f64::consts::PI);
            let big_e = newton::<5>(|x| (x - e * det::sin(x) - m, 1.0 - e * det::cos(x)), m);
            r.f(big_e);
        }
        put("newton/e_sin5".into(), r);
    }

    // ---- the integer noise basis ----
    {
        let mut s = Stream(0xD37_3000);
        let mut h = Row::new();
        let mut v = Row::new();
        for _ in 0..8192 {
            let (x, y, z, seed) = (s.next() as i64, s.next() as i64, s.next() as i64, s.next());
            h.u(hash3(x, y, z, seed));
            v.f(value_noise_i(IVec3::new(x >> 20, y >> 40, z), seed));
        }
        put("noise/hash3".into(), h);
        put("noise/value".into(), v);
        for scale in [-24i8, -8, 0, 4, 12, 24, 40, 56] {
            let mut r = Row::new();
            for origin in [0i64, -7_000_001, 1 << 40, -(1 << 62)] {
                for z in 0..4 {
                    for y in 0..16 {
                        for x in 0..16 {
                            let p = IVec3::new(origin + x * 37, origin - y * 11, z - origin);
                            r.f(simplex_i(p, scale, 0x5EED ^ scale as u64));
                        }
                    }
                }
            }
            put(format!("noise/simplex/s{scale}"), r);
        }
    }

    // ---- 256 noise chunks: 8 body seeds x 4 LODs x 8 chunks of 8^3 samples ----
    for body in 0..8u64 {
        let body_seed = hash3(body as i64, 0, 0, 0xB0D1);
        for lod in 0..4u32 {
            for chunk in 0..8i64 {
                let step = 1i64 << lod;
                let origin = IVec3::new(
                    (chunk * 8) << lod,
                    (hash3(body as i64, chunk, 1, 2) as i64 >> 40) << lod,
                    (1i64 << 24) * body as i64,
                );
                let mut r = Row::new();
                let mut samples = Vec::with_capacity(512);
                for z in 0..8 {
                    for y in 0..8 {
                        for x in 0..8 {
                            let p = IVec3::new(
                                origin.x + x * step,
                                origin.y + y * step,
                                origin.z + z * step,
                            );
                            let v = fbm_i(p, 10, 6, body_seed);
                            r.f(v);
                            samples.push(v);
                        }
                    }
                }
                r.f(tree_sum(&samples));
                put(format!("chunk/b{body}/lod{lod}/c{chunk}"), r);
            }
        }
    }
    two_d_rows(&mut rows);
    rows
}

/// Ch.35 (WP-U15): the 2D pipeline's deterministic parts — whole physics simulations,
/// autotiling, atlas packing, shadow polygons, spline geometry, a posed cutout rig with IK,
/// particles, navigation paths and an Aseprite round trip. None of them touches `fbm_i`, so
/// the mutate-det control leaves every `2d/*` row unchanged.
fn two_d_rows(rows: &mut BTreeMap<String, String>) {
    use forge_2d::physics::{BodyDef, BodyKind, ColliderDef, PhysicsWorld2d, Shape};
    use forge_2d::{DVec2, FrameId, FramePos2};
    let f = FrameId(0);
    let at = |x: f64, y: f64| FramePos2::new(f, DVec2::new(x, y));
    // Physics: a box pyramid, then a mixed pile (circles, capsules, triangles, a joint chain).
    let mut w = PhysicsWorld2d::new(f);
    let g = w
        .add_body(&BodyDef::new(BodyKind::Static, at(0.0, -0.5)))
        .expect("ground");
    w.add_collider(
        g,
        ColliderDef::new(Shape::Box {
            half: DVec2::new(40.0, 0.5),
        }),
    )
    .expect("ground box");
    for row in 0..8 {
        for i in 0..8 - row {
            let x = (f64::from(i) - f64::from(7 - row) * 0.5) * 1.02;
            let b = w
                .add_body(&BodyDef::new(
                    BodyKind::Dynamic,
                    at(x, 0.5 + f64::from(row)),
                ))
                .expect("box");
            w.add_collider(
                b,
                ColliderDef::new(Shape::Box {
                    half: DVec2::new(0.5, 0.5),
                }),
            )
            .expect("box collider");
        }
    }
    let mut r = Row::new();
    for step in 0..300 {
        w.step();
        if step % 30 == 29 {
            for b in w.state_bits() {
                r.u(b);
            }
        }
    }
    put_row(rows, "2d/physics/pyramid".into(), &r);
    let mut prev = None;
    for i in 0..40u32 {
        let b = w
            .add_body(&BodyDef {
                angle: f64::from(i) * 0.37,
                angular_velocity: 0.5,
                ..BodyDef::new(
                    BodyKind::Dynamic,
                    at(-8.0 + f64::from(i % 8) * 2.0, 12.0 + f64::from(i / 8) * 1.4),
                )
            })
            .expect("b");
        let shape = match i % 3 {
            0 => Shape::Circle { radius: 0.3 },
            1 => Shape::Capsule {
                half_height: 0.25,
                radius: 0.2,
            },
            _ => Shape::Polygon {
                verts: vec![
                    DVec2::new(-0.4, -0.3),
                    DVec2::new(0.4, -0.3),
                    DVec2::new(0.1, 0.45),
                ],
                radius: 0.02,
            },
        };
        let mut c = ColliderDef::new(shape);
        c.restitution = 0.3;
        c.friction = 0.4;
        w.add_collider(b, c).expect("c");
        if i >= 32 {
            if let Some(p) = prev {
                w.add_revolute(
                    p,
                    b,
                    at(-8.0 + f64::from(i % 8) * 2.0 - 1.0, 12.0 + 4.0 * 1.4),
                )
                .expect("joint");
            }
            prev = Some(b);
        }
    }
    let mut r = Row::new();
    for step in 0..360 {
        w.step();
        if step % 40 == 39 {
            for b in w.state_bits() {
                r.u(b);
            }
        }
    }
    put_row(rows, "2d/physics/mixed".into(), &r);
    // Autotiling over a scattered pattern.
    {
        use forge_2d::tilemap::{Cell, Tilemap, blob47};
        let ts = forge_2d::scenes::tileset(forge_2d::sprite::TextureId(0));
        let mut m = Tilemap::new(at(0.0, 0.0), DVec2::new(1.0, 1.0), ts);
        let l = m.add_layer("l", 0, true);
        let mut s = Stream(0x2D_0001);
        for _ in 0..3000 {
            let (x, y) = ((s.next() % 64) as i32 - 32, (s.next() % 64) as i32 - 32);
            m.set(l, x, y, Cell::Terrain(0)).expect("set");
        }
        let mut r = Row::new();
        for y in -33..33 {
            for x in -33..33 {
                r.u(m
                    .resolve(l, x, y)
                    .expect("resolve")
                    .map_or(u64::MAX, u64::from));
            }
        }
        for (x, y, w, h) in m.solid_rects() {
            for v in [x, y, w, h] {
                r.u(v as u64);
            }
        }
        r.u(blob47().len() as u64);
        put_row(rows, "2d/tilemap/autotile".into(), &r);
    }
    // Atlas packing.
    {
        let mut s = Stream(0x2D_0002);
        let sizes: Vec<(u32, u32, u32)> = (0..300)
            .map(|i| (i, 1 + (s.next() % 90) as u32, 1 + (s.next() % 90) as u32))
            .collect();
        let opts = forge_2d::atlas::AtlasOptions {
            page_w: 512,
            page_h: 512,
            padding: 2,
            extrude: 1,
        };
        let (regions, pages) = forge_2d::atlas::pack_rects(&sizes, &opts).expect("pack");
        let mut r = Row::new();
        r.u(u64::from(pages));
        for (k, g) in regions {
            for v in [k, g.page, g.x, g.y, g.w, g.h] {
                r.u(u64::from(v));
            }
        }
        put_row(rows, "2d/atlas/pack".into(), &r);
    }
    // Shadow polygons.
    {
        let mut s = Stream(0x2D_0003);
        let segs: Vec<(DVec2, DVec2)> = (0..120)
            .map(|_| {
                let a = DVec2::new(s.uniform(-20.0, 20.0), s.uniform(-20.0, 20.0));
                (
                    a,
                    a + DVec2::new(s.uniform(-3.0, 3.0), s.uniform(-3.0, 3.0)),
                )
            })
            .collect();
        let mut r = Row::new();
        let mut out = Vec::new();
        for _ in 0..24 {
            let c = DVec2::new(s.uniform(-15.0, 15.0), s.uniform(-15.0, 15.0));
            forge_2d::light::visibility(c, s.uniform(3.0, 12.0), &segs, &mut out);
            r.u(out.len() as u64);
            for p in &out {
                r.f(p.x);
                r.f(p.y);
            }
        }
        put_row(rows, "2d/light/visibility".into(), &r);
    }
    // Spline geometry.
    {
        let t = forge_2d::sprite::TextureId(0);
        let shape = forge_2d::spline::SplineShape::closed(
            at(1.0, 2.0),
            vec![
                DVec2::new(-9.0, -5.0),
                DVec2::new(9.0, -5.0),
                DVec2::new(9.0, -1.0),
                DVec2::new(4.0, 0.2),
                DVec2::new(0.0, -0.8),
                DVec2::new(-4.0, 0.6),
                DVec2::new(-9.0, -1.0),
            ],
            t,
            t,
        );
        let mut r = Row::new();
        for m in [shape.fill().expect("fill"), shape.edge().expect("edge")] {
            for v in &m.verts {
                r.f(v.offset.x);
                r.f(v.offset.y);
                r.f(v.uv.x);
                r.f(v.uv.y);
            }
            for i in &m.indices {
                r.u(u64::from(*i));
            }
        }
        put_row(rows, "2d/spline/hill".into(), &r);
    }
    // A cutout rig: its walk clip sampled, blended and corrected by two-bone IK.
    {
        use forge_2d::math::Xform2;
        use forge_2d::skeleton::Skeleton2d;
        let (sk, _, clip) = forge_2d::scenes::cutout();
        let mut r = Row::new();
        for k in 0..30u64 {
            let t = k * 37_000;
            let a = sk.sample(&clip, t);
            let b = sk.sample(&clip, t + 400_000);
            let mut p = Skeleton2d::blend(&a, &b, 0.25);
            sk.two_bone_ik(
                &mut p,
                &Xform2::IDENTITY,
                3,
                4,
                DVec2::new(-0.2, -0.8 + 0.01 * k as f64),
                false,
            )
            .expect("ik");
            for x in sk.world(&p, &Xform2::IDENTITY) {
                r.f(x.translation.x);
                r.f(x.translation.y);
                r.f(x.rot.c);
                r.f(x.rot.s);
            }
        }
        put_row(rows, "2d/skeleton/walk".into(), &r);
    }
    // Particles.
    {
        use forge_2d::particles::{Emitter, ParticleSystem};
        let mut ps = ParticleSystem::new(
            Emitter {
                rate: 300.0,
                bursts: vec![(0.5, 200)],
                radius: 0.5,
                ..Emitter::default()
            },
            at(0.0, 0.0),
            0x2D_0004,
        )
        .expect("particles");
        let mut r = Row::new();
        for step in 0..180 {
            ps.step(1.0 / 60.0);
            if step % 20 == 19 {
                for b in ps.state_bits() {
                    r.u(b);
                }
            }
        }
        put_row(rows, "2d/particles/fountain".into(), &r);
    }
    // Navigation.
    {
        use forge_2d::nav::NavGrid;
        let mut g = NavGrid::open(at(0.0, 0.0), DVec2::new(1.0, 1.0), 0, 0, 64, 48);
        let mut s = Stream(0x2D_0005);
        for _ in 0..900 {
            g.set((s.next() % 64) as i32, (s.next() % 48) as i32, false);
        }
        let mut r = Row::new();
        for _ in 0..20 {
            let (a, b) = ((s.next() % 64) as i32, (s.next() % 48) as i32);
            let (c, d) = ((s.next() % 64) as i32, (s.next() % 48) as i32);
            g.set(a, b, true);
            g.set(c, d, true);
            match g
                .find_path(g.center_of(a, b), g.center_of(c, d))
                .expect("path")
            {
                Some(p) => {
                    r.u(p.len() as u64);
                    for q in p {
                        r.f(q.local.x);
                        r.f(q.local.y);
                    }
                }
                None => r.u(u64::MAX),
            }
        }
        put_row(rows, "2d/nav/grid".into(), &r);
    }
    // Aseprite: a written file decodes to the same composite and sheet.
    {
        use forge_2d::aseprite::{AseTag, AseWrite, decode, to_sheet, write};
        use forge_2d::atlas::Image;
        let mut s = Stream(0x2D_0006);
        let mut img = |w: u32, h: u32| {
            let mut i = Image::filled(w, h, [0, 0, 0, 0]);
            for p in i.rgba.chunks_mut(4) {
                let v = s.next();
                p.copy_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8]);
            }
            i
        };
        let doc = AseWrite {
            w: 24,
            h: 16,
            layers: vec![("a".into(), true, 255), ("b".into(), true, 150)],
            frames: vec![
                (80, vec![(0, 0, 0, img(24, 16)), (1, 3, 2, img(10, 9))]),
                (120, vec![(0, -4, 5, img(24, 16)), (1, 12, 0, img(8, 8))]),
            ],
            tags: vec![AseTag {
                name: "t".into(),
                from: 0,
                to: 1,
                direction: 2,
            }],
            compress: true,
        };
        let d = decode(&write(&doc)).expect("decode");
        let (sheet, _, anims) = to_sheet(&d);
        let mut r = Row::new();
        for b in &sheet.rgba {
            r.u(u64::from(*b));
        }
        for a in anims {
            for fr in a.frames {
                r.u(u64::from(fr));
            }
        }
        put_row(rows, "2d/aseprite/roundtrip".into(), &r);
    }
    // WP-U16: the 2D sample game's scripted run (M4-12) — the shipped game's whole
    // trajectory, its state folded in every 30 steps: hero, contacts, particles, coins, clocks.
    {
        let mut r = Row::new();
        let script = forge_2d_game::demo_script(forge_2d_game::SCRIPT_STEPS as u64);
        forge_2d_game::play_script(&script, forge_2d_game::SCRIPT_STEPS, |i, g| {
            if i % 30 == 29 {
                for b in g.state_bits() {
                    r.u(b);
                }
            }
        })
        .expect("the sample game's scripted run");
        put_row(rows, "2d/sample-game/run".into(), &r);
    }
}

fn put_row(rows: &mut BTreeMap<String, String>, name: String, row: &Row) {
    rows.insert(name, row.hex());
}

fn golden_path() -> PathBuf {
    forge_tests::workspace_root().join("tests").join(GOLDEN)
}

fn parse_golden(src: &str) -> BTreeMap<String, String> {
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (k, v) = l.split_once(' ')?;
            Some((k.to_string(), v.trim().to_string()))
        })
        .collect()
}

fn render_golden(rows: &BTreeMap<String, String>) -> String {
    let mut s = String::from(
        "# Ch.3.4 golden hashes (I2). BLAKE3 of each corpus row; see\n\
         # tests/determinism/test_cross_platform_hash.rs. A change here needs a plan amendment\n\
         # and a one-line reason in docs/adr/. Rows are only ever appended.\n",
    );
    for (k, v) in rows {
        s.push_str(&format!("{k} {v}\n"));
    }
    s
}

/// Row names that differ, are missing, or are unexpected.
fn diff(want: &BTreeMap<String, String>, got: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in got {
        match want.get(k) {
            None => out.push(format!("{k}: not in golden")),
            Some(w) if w != v => out.push(format!("{k}: {w} -> {v}")),
            _ => {}
        }
    }
    for k in want.keys() {
        if !got.contains_key(k) {
            out.push(format!("{k}: in golden but no longer generated"));
        }
    }
    out
}

#[test]
fn golden_hashes_match() {
    let got = corpus();
    let src = std::fs::read_to_string(golden_path()).unwrap_or_default();
    let want = parse_golden(&src);
    let d = diff(&want, &got);
    if !d.is_empty() {
        panic!(
            "{MISMATCH}: {} of {} rows differ (mutate-det build: {}):\n{}\n\n\
             ---- regenerated {GOLDEN} (commit only with a plan amendment + ADR line) ----\n{}",
            d.len(),
            got.len(),
            forge_num::MUTATE_DET,
            d.join("\n"),
            render_golden(&got)
        );
    }
    assert_eq!(got.len(), want.len());
    // One line to compare legs by eye (docs/evidence/linux/C-determinism-linux-leg.md): the
    // digest of the corpus as COMPUTED on this platform, independent of the golden file's
    // line endings in the working tree.
    println!(
        "I2 corpus on {}-{}: {} rows, BLAKE3 {}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        got.len(),
        blake3::hash(render_golden(&got).as_bytes()).to_hex()
    );
}

#[test]
fn the_corpus_is_stable_within_a_run() {
    // Generation is pure: evaluating the corpus twice gives the same hashes.
    assert_eq!(corpus(), corpus());
}

#[test]
fn diff_catches_every_kind_of_difference() {
    let mut a = BTreeMap::new();
    a.insert("x".to_string(), "1".to_string());
    a.insert("y".to_string(), "2".to_string());
    let mut b = a.clone();
    assert!(diff(&a, &b).is_empty());
    b.insert("y".into(), "3".into());
    b.insert("z".into(), "4".into());
    b.remove("x");
    assert_eq!(diff(&a, &b).len(), 3);
}

/// W2 positive control: the real `mutate-det` build of this test must FAIL, on exactly the
/// rows that consume `fbm_i` octave 0 — and on no other row.
#[test]
fn positive_control_mutate_det_build_fails() {
    if forge_num::MUTATE_DET {
        return; // we ARE the mutated child; never recurse
    }
    let root = forge_tests::workspace_root();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("mutate-det");
    let out = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "test",
            "--locked",
            "-p",
            "forge-tests",
            "--test",
            "test_cross_platform_hash",
            "--features",
            "mutate-det",
            "--",
            "--exact",
            "golden_hashes_match",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&root)
        .output()
        .expect("cannot run cargo for the mutate-det control");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "the mutate-det build PASSED the golden gate: a 1-ulp perturbation went undetected, \
         so the gate guards nothing (W2).\n{text}"
    );
    assert!(
        text.contains(MISMATCH) && text.contains("mutate-det build: true"),
        "the mutate-det build failed, but not on the golden comparison:\n{text}"
    );
    // Specific: every mismatched row is a noise-chunk row (the consumers of fbm_i here).
    let report = text.split("---- regenerated").next().unwrap_or("");
    let bad: Vec<&str> = report
        .lines()
        .filter(|l| l.contains(" -> "))
        .map(|l| l.split(':').next().unwrap_or(""))
        .collect();
    assert!(!bad.is_empty(), "no mismatched rows reported:\n{text}");
    assert!(
        bad.iter().all(|r| r.starts_with("chunk/")),
        "rows outside fbm_i changed under mutate-det: {bad:?}"
    );
}
