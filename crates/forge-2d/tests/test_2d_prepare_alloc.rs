//! `test_2d_prepare_alloc` — a steady 2D frame's last mile allocates **nothing** (owner rule
//! 2; Ch.35 §35.5): culling, sorting, batching, 32 shadow polygons and narrowing ~5,000
//! sprites into the upload buffers all reuse the buffers of the previous frame.
//!
//! Positive control (W2): the same frame prepared into a **fresh** `Prepared` every time
//! allocates — proving the counter sees the last mile's buffers, so the zero is reuse, not a
//! blind counter.

use forge_2d::render::last_mile::Prepared;
use forge_2d::scenes::{self, SceneTextures};
use forge_2d::sprite::TextureId;

fn frame() -> forge_2d::sprite::Frame2d {
    let t = SceneTextures {
        tiles: TextureId(1),
        atlas: TextureId(2),
        bricks: TextureId(3),
        white: TextureId(0),
    };
    let atlas = scenes::sprite_atlas().expect("atlas");
    scenes::perf_scene(&t, &atlas).expect("perf scene")
}

fn allocations(reuse: bool) -> u64 {
    let f = frame();
    let mut p = Prepared::default();
    for _ in 0..3 {
        p.fill(&f, 1920, 1080, &|_| true, false).expect("fill");
    }
    let info = allocation_counter::measure(|| {
        for _ in 0..5 {
            if reuse {
                p.fill(&f, 1920, 1080, &|_| true, false).expect("fill");
            } else {
                let mut fresh = Prepared::default();
                fresh.fill(&f, 1920, 1080, &|_| true, false).expect("fill");
            }
        }
    });
    info.count_total
}

#[test]
fn a_steady_2d_frame_prepares_without_allocating() {
    let n = allocations(true);
    assert_eq!(n, 0, "five steady 2D last miles made {n} heap allocations");
}

#[test]
fn positive_control_a_fresh_prepared_is_counted() {
    let n = allocations(false);
    assert!(
        n > 0,
        "a fresh Prepared must allocate: the counter is not blind ({n})"
    );
}
