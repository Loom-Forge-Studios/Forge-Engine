//! `test_2d_render` — the 2D render path's frame contract (Ch.35 §35.1-35.2):
//!
//! * **Batching**: the perf scene at 1920x1080 (three tile layers, 600 atlas sprites, 2,000 particles, a
//!   spline shape) draws in a handful of calls, every texture run one instanced draw.
//!   Control: `Faults2d::no_batching` draws one call per sprite.
//! * **Steady state**: after the first frame, a frame of the same size reuses the compiled
//!   graph and allocates no GPU memory; every pass is timed where the device has
//!   timestamps; a different reference size rebuilds.
//! * **One frame tree**: a sprite in another frame is refused with `TWOD-0002`.

mod common;

use forge_2d::render::{PASS_NAMES, RenderOptions2d, Renderer2d};
use forge_2d::scenes;
use forge_2d::{Faults2d, FrameId};

const ROW: &str = "C-2d-render-steady";

fn perf_report(faults: Faults2d, frames: usize) -> Vec<forge_2d::render::Report2d> {
    let Some(pool) = common::pool(ROW) else {
        return Vec::new();
    };
    let dev = pool.primary();
    let mut opts = RenderOptions2d::new(1920, 1080);
    opts.timing = true;
    opts.faults = faults;
    let mut r = Renderer2d::new(dev, opts).expect("renderer");
    let (t, atlas) = common::textures(&mut r, dev);
    let f = scenes::perf_scene(&t, &atlas).expect("scene");
    let target = common::target(dev, 1920, 1080);
    let out = (0..frames)
        .map(|_| r.render(dev, &f, &target).expect("render"))
        .collect();
    assert!(dev.take_uncaptured_errors().is_empty());
    out
}

#[test]
fn the_perf_scene_draws_in_a_handful_of_batched_calls() {
    let reps = perf_report(Faults2d::default(), 1);
    let Some(rep) = reps.first() else { return };
    println!("{:?}", rep.prepare);
    assert!(
        rep.prepare.sprites > 2500,
        "the scene is big: {:?}",
        rep.prepare
    );
    assert!(rep.prepare.draw_calls <= 12, "batched: {:?}", rep.prepare);
    assert_eq!(rep.prepare.lights, 32);
    assert_eq!(rep.render_size, (1920, 1080));
}

#[test]
fn positive_control_without_batching_every_sprite_is_a_draw() {
    let reps = perf_report(
        Faults2d {
            no_batching: true,
            ..Faults2d::default()
        },
        1,
    );
    let Some(rep) = reps.first() else { return };
    assert!(
        rep.prepare.draw_calls >= rep.prepare.sprites,
        "{:?}",
        rep.prepare
    );
}

#[test]
fn a_steady_frame_reuses_its_graph_and_allocates_nothing() {
    let reps = perf_report(Faults2d::default(), 6);
    if reps.is_empty() {
        return;
    }
    assert!(reps[0].graph_rebuilt);
    for (i, r) in reps.iter().enumerate().skip(2) {
        assert!(!r.graph_rebuilt, "frame {i} rebuilt its graph");
        assert_eq!(r.exec.allocations, 0, "frame {i} allocated GPU memory");
        let names: Vec<&str> = r.passes.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, PASS_NAMES, "every pass runs, in order");
    }
}

#[test]
fn a_new_reference_size_rebuilds_and_a_foreign_frame_is_refused() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let mut r = Renderer2d::new(dev, RenderOptions2d::new(640, 360)).expect("renderer");
    let (t, atlas) = common::textures(&mut r, dev);
    let mut f = scenes::tiles_scene(&t, &atlas).expect("scene");
    let target = common::target(dev, 640, 360);
    assert!(r.render(dev, &f, &target).expect("render").graph_rebuilt);
    assert!(!r.render(dev, &f, &target).expect("render").graph_rebuilt);
    f.camera.reference = Some((160, 90));
    let rep = r.render(dev, &f, &target).expect("render");
    assert!(rep.graph_rebuilt);
    assert_eq!((rep.render_size, rep.scale), ((160, 90), 4));
    f.sprites[0].pos.frame = FrameId(5);
    let e = r.render(dev, &f, &target).expect_err("foreign frame");
    assert_eq!(e.code().as_str(), "TWOD-0002");
}
