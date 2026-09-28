// timed-gates: exempt(the first frame time is printed; the budget asserted is draw calls)
//! `test_ui_on_forge_gpu` — the UI renderer runs on `forge-gpu`'s adapter pool (D-3, M1-1)
//! without regressing the D-5 budgets.
//!
//! * The renderer's device is the pool's primary member (UI is latency-critical, never
//!   Tier-0 work), and its shaders went through forge-gpu's naga validation.
//! * `ui_draw_calls_batched` on the real GPU path: a 1,000-button panel draws in at most 4
//!   draw calls (the same budget as `tests/perf/test_ui_budgets.rs`, which measures the
//!   recording renderer).
//! * Idle is free: with no damage `Ui::render` issues no GPU work at all.
//! * No wgpu error escaped to the pool's uncaptured-error sink.
//!
//! Positive control (W2): `positive_control_batching_disabled_breaks_the_budget_on_the_gpu`.

use std::sync::Arc;
use std::time::Instant;

use forge_ui::geom::PhysicalSize;
use forge_ui::render::TargetId;
use forge_ui::render_wgpu::{AdapterPool, GpuContext, PoolOptions, WgpuRenderer};
use forge_ui::widgets::{Button, Container};
use forge_ui::{NodeStyle, Size, Ui, UiConfig};

fn context() -> Option<GpuContext> {
    match AdapterPool::new(&PoolOptions::default()) {
        Ok(pool) => Some(GpuContext::from_pool(Arc::new(pool), 0).expect("primary")),
        Err(e) if std::env::var("CI").is_ok_and(|v| v == "true" || v == "1") => {
            panic!("CI=true and no GPU adapter ({e}): GPU guards must not pass silently (W9)")
        }
        Err(e) => {
            println!("AWAITING(no adapter): {e}");
            None
        }
    }
}

fn button_panel(disable_batching: bool) -> Ui {
    let mut ui = Ui::new(UiConfig {
        size: Size::new(1920.0, 1400.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    ui.faults.batch.disable_batching = disable_batching;
    let panel = ui
        .add(
            ui.root(),
            "panel",
            NodeStyle::row(4.0).wrap().fill().padding(8.0),
            Container::group(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    for i in 0..1000u32 {
        ui.add(
            panel,
            forge_ui::Key::Index(i),
            NodeStyle::leaf(),
            Button::new(format!("B{i}")),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
    ui.frame(std::time::Duration::ZERO);
    ui
}

/// (draw calls of the first full frame, whether a second render did any work, ms).
fn render_panel(ctx: &GpuContext, disable_batching: bool) -> (u32, bool, f64) {
    let mut r = WgpuRenderer::from_context(ctx).expect("renderer on the pool device");
    let t = TargetId(1);
    r.add_offscreen_target(t, PhysicalSize { w: 1920, h: 1400 });
    let mut ui = button_panel(disable_batching);
    let t0 = Instant::now();
    let stats = ui
        .render(&mut r, t)
        .expect("render")
        .expect("first frame has damage");
    let _ = r.read_pixels(t).expect("readback"); // wait for the GPU
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let again = ui.render(&mut r, t).expect("render");
    (stats.draw_calls, again.is_some(), ms)
}

#[test]
fn the_ui_renders_on_the_pool_primary_within_its_budgets() {
    let Some(ctx) = context() else { return };
    assert_eq!(ctx.index, 0);
    assert!(
        ctx.device == ctx.pool.primary().device,
        "UI must render on the primary"
    );
    let (calls, idle_work, ms) = render_panel(&ctx, false);
    println!(
        "ui on forge-gpu ({}): 1,000 buttons in {calls} draw calls, first frame {ms:.2} ms incl. readback",
        ctx.adapter_name()
    );
    assert!(
        calls <= 4,
        "1,000-button panel drew in {calls} draw calls on the GPU (budget 4)"
    );
    assert!(!idle_work, "a render with no damage did GPU work");
    assert!(ctx.pool.primary().take_uncaptured_errors().is_empty());
}

#[test]
fn positive_control_batching_disabled_breaks_the_budget_on_the_gpu() {
    let Some(ctx) = context() else { return };
    let (calls, _, _) = render_panel(&ctx, true);
    assert!(
        calls > 4,
        "with batching disabled the GPU budget still held ({calls})"
    );
}
