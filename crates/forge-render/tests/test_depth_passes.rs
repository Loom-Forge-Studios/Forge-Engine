//! `test_depth_passes` — the renderer draws the depth passes the frame resolver names
//! (Ch.2.4, ADR 0060).
//!
//! * **The world frame is one pass.** Through `WorldFrame` a frame prepares exactly one depth
//!   pass, `scene` (1 cm – 1e4 km, reversed-Z, pass-uniform block 0): a cube 5 m away and one
//!   50 km away are both drawn in it, one 2e4 km away is culled (the world has no sky
//!   sprites), and the frame graph runs `shell.sky`, `scene` and `tonemap`.
//! * **A resolver's view depth is honoured.** A resolver naming two passes gets two, back to
//!   front, the nearest in block 0, an object straddling their boundary drawn in both.
//! * **At most three passes.** A view depth of four passes is refused (`RENDER-0003`), never
//!   drawn with a pass-uniform block it does not have.
//!
//! Positive control (W2): `positive_control_an_overridden_layout_ignores_the_resolver` — the
//! same two-pass resolver with `RenderOptions::depth` forcing the world's one pass prepares
//! one pass, so the two-pass assertion above sees the resolver and not a constant.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use forge_frames::testing::FixedFrames;
use forge_frames::{
    DVec3, DepthSpan, FrameId, FramePos, FrameResolver, Tick, ViewDepth, WorldFrame,
};
use forge_render::{
    Camera, DepthSetup, Instance, Material, MeshData, PreparedFrame, RenderOptions, Renderer, Scene,
};

const ROW: &str = "C-depth-from-resolver";
const W: u32 = 320;
const H: u32 = 200;

const TWO: [DepthSpan; 2] = [
    DepthSpan {
        near: 100.0,
        far: 1e6,
        pass: "test.far",
        attachment: "depth.test.far",
    },
    DepthSpan {
        near: ViewDepth::NEAREST,
        far: 100.0,
        pass: "test.near",
        attachment: "depth.test.near",
    },
];

const FOUR: [DepthSpan; 4] = [
    DepthSpan {
        near: 1e6,
        far: 1e7,
        pass: "a",
        attachment: "depth.a",
    },
    DepthSpan {
        near: 1e3,
        far: 1e6,
        pass: "b",
        attachment: "depth.b",
    },
    DepthSpan {
        near: 10.0,
        far: 1e3,
        pass: "c",
        attachment: "depth.c",
    },
    DepthSpan {
        near: ViewDepth::NEAREST,
        far: 10.0,
        pass: "d",
        attachment: "depth.d",
    },
];

/// Cubes straight ahead at `distances` (metres), sized to a tenth of their distance.
fn scene(r: &mut Renderer, distances: &[f64]) -> (Scene, Camera) {
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let m = r.add_material(Material::unlit([1.0, 1.0, 1.0]));
    let mut s = Scene::default();
    for &d in distances {
        s.instances.push(
            Instance::new(
                FramePos::new(FrameId::WORLD, DVec3::new(0.0, 0.0, -d)),
                cube,
                m,
            )
            .scaled(0.1 * d),
        );
    }
    let cam = Camera::looking(
        FramePos::origin_of(FrameId::WORLD),
        DVec3::new(0.0, 0.0, -1.0),
        DVec3::Y,
        1.0,
    )
    .expect("camera");
    (s, cam)
}

fn prepare(
    depth: DepthSetup,
    frames: &dyn FrameResolver,
    distances: &[f64],
) -> Option<Result<PreparedFrame, forge_render::RenderError>> {
    let pool = common::pool(ROW)?;
    let dev = pool.primary();
    let opts = RenderOptions {
        depth,
        ..RenderOptions::new(W, H)
    };
    let mut r = Renderer::new(dev, opts).expect("renderer");
    let (s, cam) = scene(&mut r, distances);
    Some(r.prepare(&s, &cam, frames, Tick(0)))
}

#[test]
fn the_world_frame_is_drawn_in_one_pass() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let mut r = Renderer::new(dev, RenderOptions::new(W, H)).expect("renderer");
    let (s, cam) = scene(&mut r, &[5.0, 5.0e4, 2.0e7]);
    let f = r.prepare(&s, &cam, &WorldFrame, Tick(0)).expect("prepare");
    assert_eq!(f.passes.len(), 1, "{:?}", f.passes);
    let p = &f.passes[0];
    assert_eq!((p.span.pass, p.block), ("scene", 0));
    assert_eq!(p.instances(), 2, "the cubes at 5 m and 50 km");
    assert_eq!(
        f.stats.culled, 1,
        "the cube at 2e4 km is past the far plane"
    );
    assert_eq!(
        f.stats.sprites, 0,
        "a single-frame world has no sky sprites"
    );
    assert_eq!(f.view_depth(), ViewDepth::WORLD);
    let plan = r.plan(&f).expect("plan");
    assert_eq!(forge_gpu::verify_plan(&plan), Vec::<String>::new());
    assert_eq!(plan.order_names(), ["shell.sky", "scene", "tonemap"]);
    let target = common::target(dev, W, H);
    r.render(dev, &f, &target).expect("render");
    assert!(dev.take_uncaptured_errors().is_empty());
}

#[test]
fn a_resolvers_view_depth_is_honoured() {
    let frames = FixedFrames::new().with_view_depth(ViewDepth {
        passes: &TWO,
        sprites_beyond: None,
    });
    // 5 m (near), 100 m +- 10 m (both), 1e4 m (far).
    let Some(f) = prepare(DepthSetup::default(), &frames, &[5.0, 100.0, 1.0e4]) else {
        return;
    };
    let f = f.expect("prepare");
    let got: Vec<(&str, usize, u32)> = f
        .passes
        .iter()
        .map(|p| (p.span.pass, p.block, p.instances()))
        .collect();
    assert_eq!(got, [("test.far", 1, 2), ("test.near", 0, 2)], "{got:?}");
}

#[test]
fn more_passes_than_blocks_are_refused() {
    let frames = FixedFrames::new().with_view_depth(ViewDepth {
        passes: &FOUR,
        sprites_beyond: None,
    });
    let Some(r) = prepare(DepthSetup::default(), &frames, &[5.0]) else {
        return;
    };
    let e = r.expect_err("four passes");
    assert_eq!(e.code().as_str(), "RENDER-0003", "{e}");
}

#[test]
fn positive_control_an_overridden_layout_ignores_the_resolver() {
    let frames = FixedFrames::new().with_view_depth(ViewDepth {
        passes: &TWO,
        sprites_beyond: None,
    });
    let forced = DepthSetup {
        layout: Some(ViewDepth::WORLD),
        reversed: true,
    };
    let Some(f) = prepare(forced, &frames, &[5.0, 100.0, 1.0e4]) else {
        return;
    };
    let f = f.expect("prepare");
    assert_eq!(
        f.passes.len(),
        1,
        "the forced world layout, not the resolver's two"
    );
}
