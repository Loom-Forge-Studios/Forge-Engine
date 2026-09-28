//! The retained batch cache and the mesh pipeline (Ch.21 §21.11, §21.13; ADR 0013).
//!
//! * After any sequence of interactions the retained, patched batch list draws exactly
//!   what a from-scratch batch of the display list draws (same instances in the same
//!   order, same draw calls) — the cache is an optimisation, never a different picture.
//! * Rounded panel backgrounds carry the theme radius.
//! * Meshes (the second pipeline) draw on the GPU, anti-aliased, and a changing curve
//!   patches in place without re-walking the tree.

use std::time::Duration;

use forge_ui::gallery::{self, GalleryFaults};
use forge_ui::geom::PhysicalSize;
use forge_ui::render::{BatchKind, BatchList, Instance, PathBuilder, Primitive, TargetId};
use forge_ui::testing::Harness;
use forge_ui::widget::PaintCx;
use forge_ui::{
    ColorRole, Handled, KeyCode, Modifiers, NodeStyle, Point, Role, Signal, Size, Ui, UiConfig,
    UiEvent, Widget,
};

/// The drawn content of a list: non-slack instances in order, and draw structure.
fn drawn(list: &BatchList) -> (Vec<Instance>, usize) {
    let mut out = Vec::new();
    for b in &list.batches {
        if let BatchKind::Instances { start, count } = b.kind {
            for i in &list.instances[start as usize..(start + count) as usize] {
                if *i != Instance::default() {
                    out.push(*i);
                }
            }
        }
    }
    (out, list.draw_calls())
}

fn assert_cache_matches(h: &mut Harness, what: &str) {
    h.full_frame();
    let cached = drawn(h.ui.batch_list());
    let reference = drawn(&h.ui.reference_batches());
    assert_eq!(cached.1, reference.1, "{what}: draw calls differ");
    assert_eq!(
        cached.0.len(),
        reference.0.len(),
        "{what}: instance counts differ"
    );
    assert!(cached.0 == reference.0, "{what}: instances differ");
}

#[test]
fn patched_batches_draw_exactly_what_a_full_build_draws() {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_cache_matches(&mut h, "initial");
    for id in [g.save, g.cancel, g.close_icon, g.checkbox, g.slider] {
        if let Some(r) = h.ui.rect(id) {
            h.move_to(r.center());
            h.step();
        }
    }
    assert_cache_matches(&mut h, "after hovers");
    h.ui.set_focus(Some(g.text_field), true);
    h.type_text("Hello, patched world");
    h.step();
    assert_cache_matches(&mut h, "after typing");
    h.key(KeyCode::Char('a'), Modifiers::CTRL);
    h.key(KeyCode::Backspace, Modifiers::NONE);
    h.step();
    assert_cache_matches(&mut h, "after deleting");
    h.ui.set_focus(Some(g.tabs), true);
    h.key(KeyCode::Right, Modifiers::NONE);
    h.settle();
    assert_cache_matches(&mut h, "after a tab switch");
    h.ui.set_theme(forge_ui::Theme::light());
    h.settle();
    assert_cache_matches(&mut h, "after a theme switch");
}

#[test]
fn rounded_panels_carry_the_theme_radius() {
    let mut ui = Ui::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut ui, GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    ui.frame(Duration::ZERO);
    let lg = ui.theme().radius.lg;
    let panel_rect = ui.rect(g.panels[0]).unwrap_or_default();
    let found = ui.display_list().into_iter().any(|p| {
        matches!(p, Primitive::Quad { rect, radii, .. } if rect == panel_rect && radii.tl == lg && radii.br == lg)
    });
    assert!(found, "the panel background is not rounded with radius.lg");
}

/// A diagonal stroke whose end follows a signal (a stand-in for a dragged curve key).
struct Stroke {
    end: Signal<f32>,
}

impl Widget for Stroke {
    fn role(&self) -> Role {
        Role::Canvas
    }
    fn bind(&self, b: &mut forge_ui::widget::Binder) {
        b.watch(Some(self.end.any()), forge_ui::Dirty::PAINT);
    }
    fn event(&mut self, _cx: &mut forge_ui::widget::EventCx, _ev: &UiEvent) -> Handled {
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let mut p = PathBuilder::new();
        p.move_to(Point::new(r.x + 10.0, r.y + 10.0))
            .line_to(Point::new(r.x + self.end.get(cx.rt()), r.bottom() - 10.0));
        cx.stroke_path(&p.build(), 4.0, ColorRole::Accent, ColorRole::BgBase);
    }
}

#[test]
fn a_changing_mesh_patches_in_place() {
    let mut h = Harness::new(UiConfig {
        size: Size::new(200.0, 200.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let end = h.ui.rt_mut().signal(150.0f32);
    let still = h.ui.rt_mut().signal(180.0f32);
    h.ui.add(
        h.ui.root(),
        "stroke",
        NodeStyle::leaf().size(200.0, 100.0),
        Stroke { end },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    h.ui.add(
        h.ui.root(),
        "still",
        NodeStyle::leaf().size(200.0, 100.0),
        Stroke { end: still },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let s = h.full_frame().unwrap_or_default();
    assert!(s.mesh_draws >= 2, "no mesh draws");
    let total = h.ui.batch_list().mesh_vertices.len() as u32;
    for x in [120.0f32, 90.0, 60.0] {
        end.set(h.ui.rt_mut(), x);
        let s = h.step().unwrap_or_default();
        assert!(
            !h.ui.stats().last_reassembled,
            "a moving curve re-walked the tree"
        );
        // Ranged upload: only the moving stroke's vertices go to the GPU, never the
        // still one's (and never the whole mesh buffer).
        assert!(
            s.mesh_vertices_uploaded > 0 && s.mesh_vertices_uploaded < total,
            "uploaded {} of {total} mesh vertices for one dirty mesh",
            s.mesh_vertices_uploaded
        );
        assert_cache_matches(&mut h, "after moving the curve");
    }
    // An idle frame uploads no mesh data at all.
    let s = h.full_frame().unwrap_or_default();
    assert_eq!(
        s.mesh_vertices_uploaded, 0,
        "an unchanged frame re-uploaded meshes"
    );
}

/// The per-render scratch lists are retained: after a frame that re-batched many slices,
/// their capacity stays for the next frame (no per-frame reallocation of the changed list
/// or of the repaint list).
#[test]
fn batch_scratch_lists_keep_their_capacity() {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    for id in [g.save, g.cancel, g.close_icon, g.checkbox, g.slider] {
        if let Some(r) = h.ui.rect(id) {
            h.move_to(r.center());
            h.step();
        }
    }
    let (repaint_cap, changed_cap) = h.ui.batch_scratch_capacity();
    assert!(
        repaint_cap > 0 && changed_cap > 0,
        "{repaint_cap} {changed_cap}"
    );
    h.move_to(Point::new(-10.0, -10.0));
    h.step();
    let after = h.ui.batch_scratch_capacity();
    assert!(
        after.0 >= repaint_cap && after.1 >= changed_cap,
        "scratch capacity was dropped: {after:?} < {:?}",
        (repaint_cap, changed_cap)
    );
}

#[test]
fn mesh_strokes_draw_on_the_gpu() {
    use forge_ui::render_wgpu::{GpuContext, WgpuRenderer};
    let ctx = match GpuContext::headless(false).or_else(|_| GpuContext::headless(true)) {
        Ok(c) => c,
        Err(e) => {
            println!("mesh_strokes_draw_on_the_gpu: AWAITING(no adapter: {e})");
            return;
        }
    };
    let mut ui = Ui::new(UiConfig {
        size: Size::new(100.0, 100.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let end = ui.rt_mut().signal(90.0f32);
    ui.add(
        ui.root(),
        "stroke",
        NodeStyle::leaf().size(100.0, 100.0),
        Stroke { end },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    ui.frame(Duration::ZERO);
    let mut r = WgpuRenderer::from_context(&ctx).expect("renderer");
    let t = TargetId(9);
    r.add_offscreen_target(t, PhysicalSize { w: 100, h: 100 });
    let stats = ui
        .render(&mut r, t)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_default();
    assert!(stats.mesh_draws >= 1);
    let (w, _, px) = r.read_pixels(t).unwrap_or_else(|e| panic!("{e}"));
    let at = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    let bg = at(90, 10); // far from the line
    let on = at(50, 50); // the line from (10,10) to (90,90) passes here
    assert_ne!(on, bg, "the stroke did not draw");
    let accent = forge_ui::Theme::dark().color(ColorRole::Accent);
    let want = [
        (accent.r * 255.0).round() as u8,
        (accent.g * 255.0).round() as u8,
        (accent.b * 255.0).round() as u8,
    ];
    assert!(
        on.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 3),
        "line centre {on:?} is not the accent {want:?}"
    );
    // Anti-aliased: some pixel at the stroke edge is a blend (neither bg nor accent).
    let edge_blend = (40..60u32).any(|x| {
        let p = at(x, 50);
        p != bg && p.iter().zip(want).any(|(a, b)| a.abs_diff(b) > 3)
    });
    assert!(edge_blend, "the stroke edge is not anti-aliased");
}
