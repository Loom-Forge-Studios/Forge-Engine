//! `test_prepare_alloc` — a steady-state frame's CPU preparation allocates **nothing**
//! (owner rule 2; Ch.10 §10.9).
//!
//! The representative frame of `test_render_perf` (2,501 instances, 256 lights, four
//! cascades) is prepared and recycled for three warm-up frames, then ten more frames are
//! prepared and recycled under a counting allocator (`allocation-counter`, thread-local
//! counts, so other test threads cannot pollute the figure): the count must be zero.
//! Also: a steady frame with an unchanged topology reuses the compiled frame graph.
//!
//! Positive control (W2): the same frames with the prepared frame **dropped** instead of
//! recycled must allocate — proving the counter sees preparation's buffers and that the
//! zero above is recycling, not a blind counter.

mod common;

use forge_frames::Tick;
use forge_render::{RenderOptions, Renderer};

const ROW: &str = "C-prepare-zero-alloc";

fn steady_allocations(recycle: bool) -> Option<u64> {
    let pool = common::pool(ROW)?;
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let mut r = Renderer::new(dev, RenderOptions::new(1920, 1080)).expect("renderer");
    let (scene, cam) = common::perf_scene(&mut r, root);
    for _ in 0..3 {
        let f = r.prepare(&scene, &cam, &tree, Tick(0)).expect("prepare");
        r.recycle(f);
    }
    let info = allocation_counter::measure(|| {
        for _ in 0..10 {
            let f = r.prepare(&scene, &cam, &tree, Tick(0)).expect("prepare");
            if recycle {
                r.recycle(f);
            } else {
                drop(f);
            }
        }
    });
    Some(info.count_total)
}

#[test]
fn a_steady_frame_prepares_without_allocating() {
    let Some(n) = steady_allocations(true) else {
        return;
    };
    assert_eq!(
        n, 0,
        "ten steady-state preparations made {n} heap allocations; every buffer must come from \
         the recycled frame or the renderer's scratch"
    );
}

#[test]
fn positive_control_a_dropped_frame_is_counted() {
    let Some(n) = steady_allocations(false) else {
        return;
    };
    assert!(
        n >= 10,
        "dropping each frame must force fresh buffers ({n} allocations counted): the counter \
         is blind"
    );
}

#[test]
fn a_steady_frame_reuses_the_compiled_graph() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let mut r = Renderer::new(dev, RenderOptions::new(256, 144)).expect("renderer");
    let (mut scene, cam) = common::perf_scene(&mut r, root);
    let target = common::target(dev, 256, 144);
    let rebuilt: Vec<bool> = (0..4)
        .map(|_| {
            r.render_scene(dev, &scene, &cam, &tree, Tick(0), &target)
                .expect("render")
                .graph_rebuilt
        })
        .collect();
    assert_eq!(rebuilt, [true, false, false, false]);
    // Control: a topology change (the sun stops casting: no cascade passes) rebuilds once.
    if let Some(s) = scene.sun.as_mut() {
        s.casts_shadows = false;
    }
    let again: Vec<bool> = (0..2)
        .map(|_| {
            r.render_scene(dev, &scene, &cam, &tree, Tick(0), &target)
                .expect("render")
                .graph_rebuilt
        })
        .collect();
    assert_eq!(again, [true, false]);
    assert!(dev.take_uncaptured_errors().is_empty());
}
