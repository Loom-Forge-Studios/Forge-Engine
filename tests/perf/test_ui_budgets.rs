//! D-5 UI performance budgets as named tests (Ch.21 §21.22) — the WP-U1 rows.
//!
//! Every assertion here is a **counter** (frames, wakeups, slices, shape calls, draw
//! calls, atlases), deterministic and machine-independent, so it gates every leg. The
//! loop runs under the headless harness with an **injected clock**: "10 s idle" takes no
//! wall-clock time, and the harness wakes exactly when the winit runner would (timer and
//! animation deadlines, waker calls). Millisecond figures are printed for the record; no
//! WP-U1 row has a millisecond budget (those are the virtualisation, graph and startup
//! rows of WP-U2/U8/U12).
//!
//! Each test has a positive control that must fail (W2): the property is measured by a
//! function returning `Err` when broken, and the control asserts the `Err`.
//!
//! The virtualisation rows (`ui_virtual_*_100k`, WP-U2) and `ui_graph_2k_nodes` (WP-U8) are
//! below; `ui_hierarchy_100k` (WP-U5) is in `plugins/forge-panels-scene/tests`;
//! `ui_startup_budget` (WP-U12) spawns the real binary, so it lives in
//! `tools/forge-editor-bin/tests/test_ui_startup_budget.rs`. `ui_idle_zero_redraw` here opens
//! the gallery; with every M2 panel open it is
//! `tools/forge-editor-bin/tests/test_editor_panels_audit.rs` (WP-U12).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use forge_ui::damage::{LiveCell, LiveFeed, Wake};
use forge_ui::gallery::{self, Gallery, GalleryFaults};
use forge_ui::render::kind;
use forge_ui::testing::Harness;
use forge_ui::text::AtlasConfig;
use forge_ui::text::PlaneFormat;
use forge_ui::widgets::{Button, Container, Label, LiveReadout, TextField};
use forge_ui::{KeyCode, Modifiers, NodeStyle, Rect, Theme, UiConfig, WidgetId};

fn harness(faults: GalleryFaults) -> (Harness, Gallery) {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, faults).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, g)
}

fn show_tab(h: &mut Harness, g: &Gallery, i: usize) {
    for (j, p) in g.pages.iter().enumerate() {
        h.ui.set_hidden(*p, j != i)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    h.settle();
}

// ---- ui_idle_zero_redraw (C-ui-idle, M2-20) --------------------------------------------

/// Focus a text field, let the caret timeout expire, then idle 10 s with `tab` shown.
/// Returns (frames, wakeups) during the 10 s.
fn idle_counts(faults: GalleryFaults, tab: usize, jobs_live: bool) -> (u64, u64) {
    let (mut h, g) = harness(faults);
    g.jobs_live.set(h.ui.rt_mut(), jobs_live);
    show_tab(&mut h, &g, tab);
    h.ui.set_focus(Some(g.text_field), true);
    h.type_text("idle");
    // The caret blinks for 5 s after the last input, then stays solid.
    h.advance(Duration::from_secs(6));
    let (f0, w0) = (h.frames, h.wakeups);
    h.advance(Duration::from_secs(10));
    (h.frames - f0, h.wakeups - w0)
}

fn check_idle(faults: GalleryFaults) -> Result<(), String> {
    // General tab: the spinner (live) and the live readout are both hidden.
    // Live tab: the readout is visible, its source quiescent.
    // Jobs tab: the spinner is visible but its task is not live.
    for (tab, live, what) in [
        (0, true, "hidden live panels"),
        (2, true, "live readout visible"),
        (1, false, "idle spinner visible"),
    ] {
        let (frames, wakeups) = idle_counts(faults, tab, live);
        if frames != 0 || wakeups != 0 {
            return Err(format!(
                "{what}: {frames} frames and {wakeups} wakeups over 10 s of idle"
            ));
        }
    }
    Ok(())
}

#[test]
fn ui_idle_zero_redraw() {
    check_idle(GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    // Non-vacuous: the caret really blinked before its timeout (the test waited it out).
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.set_focus(Some(g.text_field), true);
    h.type_text("x");
    let frames = h.advance(Duration::from_secs(2));
    assert!(
        frames >= 3,
        "the caret did not blink ({frames} frames in 2 s)"
    );
    assert!(
        h.ui.widget::<TextField>(g.text_field)
            .is_some_and(|f| f.is_blinking())
    );
}

#[test]
fn positive_control_spinner_in_hidden_tab_redraws() {
    let e = check_idle(GalleryFaults {
        spinner_ignores_visibility: true,
        ..GalleryFaults::default()
    })
    .expect_err("a spinner animating in a hidden tab must break the idle budget");
    assert!(e.contains("hidden live panels"), "{e}");
}

// ---- ui_live_panel_refresh_bounded (C-ui-idle, M2-20) --------------------------------

struct LiveRun {
    frames: u64,
    wakeups: u64,
    damage: Rect,
}

/// Drive the gallery's latency feed (2 Hz) for 10 s: `bump_every` of injected time the
/// producer changes the value by `step_us`.
fn run_feed(
    h: &mut Harness,
    g: &Gallery,
    bump_every: Duration,
    step_us: u64,
    secs: u64,
) -> LiveRun {
    let (f0, w0) = (h.frames, h.wakeups);
    let mut damage = Rect::ZERO;
    let n = (secs * 1000) / bump_every.as_millis().max(1) as u64;
    for _ in 0..n {
        g.latency_us.fetch_add(step_us, Ordering::Relaxed);
        g.live_cell.bump();
        let before = h.frames;
        h.advance(bump_every);
        if h.frames > before {
            for r in &h.ui.stats().last_damage {
                damage = damage.union(r);
            }
        }
    }
    LiveRun {
        frames: h.frames - f0,
        wakeups: h.wakeups - w0,
        damage,
    }
}

fn check_live(faults: forge_ui::damage::FeedFaults) -> Result<(), String> {
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.faults.feeds = faults;
    show_tab(&mut h, &g, 2);
    // Engine stopped: nothing bumps, nothing is drawn.
    let f0 = h.frames;
    h.advance(Duration::from_secs(5));
    if h.frames != f0 {
        return Err(format!("stopped: {} frames", h.frames - f0));
    }
    // Engine playing: a bump every 10 ms, a visible change every bump; ≤ 2 Hz refresh.
    let r = run_feed(&mut h, &g, Duration::from_millis(10), 1000, 10);
    if r.frames > 2 * 10 + 1 || r.frames < 10 {
        return Err(format!("playing: {} frames in 10 s at max_hz 2", r.frames));
    }
    let readout = h.ui.rect(g.readout).unwrap_or_default().outset(2.0);
    if !readout.contains_rect(&r.damage) {
        return Err(format!(
            "playing: damage {:?} escapes the readout {readout:?}",
            r.damage
        ));
    }
    // An unchanged displayed value (latency equal after rounding): no damage.
    let r = run_feed(&mut h, &g, Duration::from_millis(10), 0, 5);
    if r.frames != 0 {
        return Err(format!("equal-after-rounding: {} frames", r.frames));
    }
    // Hidden, playing: no frames and no wakeups (the waker is unregistered).
    show_tab(&mut h, &g, 0);
    let r = run_feed(&mut h, &g, Duration::from_millis(10), 1000, 5);
    if r.frames != 0 || r.wakeups != 0 {
        return Err(format!(
            "hidden: {} frames and {} wakeups",
            r.frames, r.wakeups
        ));
    }
    // Becoming visible refreshes once.
    let f0 = h.frames;
    show_tab(&mut h, &g, 2);
    h.advance(Duration::from_secs(1));
    if h.frames - f0 > 2 {
        return Err(format!("re-shown: {} frames", h.frames - f0));
    }
    // A `ui.self` feed bumped every rendered frame (the UI's own frame counter): it must
    // never wake the loop, so it never causes the next frame.
    let self_cell = LiveCell::new();
    let shown = h.ui.rt_mut().signal(String::from("0"));
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let c2 = counter.clone();
    let host = h.ui.add(
        g.pages[2],
        "ui_self",
        NodeStyle::leaf(),
        LiveReadout::new("UI frames", shown, move || {
            c2.load(Ordering::Relaxed).to_string()
        }),
    );
    let host = host.map_err(|e| e.to_string())?;
    h.ui.add_feed(
        host,
        LiveFeed {
            source: self_cell.clone(),
            max_hz: 60,
            self_ui: true,
        },
    );
    h.settle();
    let f0 = h.frames;
    // The UI drew a frame when the panel appeared: its frame counter moved once.
    counter.fetch_add(1, Ordering::Relaxed);
    self_cell.bump();
    for _ in 0..600 {
        let before = h.frames;
        h.advance(Duration::from_millis(16));
        if h.frames > before {
            counter.fetch_add(1, Ordering::Relaxed);
            self_cell.bump();
        }
    }
    if h.frames - f0 > 2 {
        return Err(format!(
            "ui.self feed: {} frames in ~10 s (it woke the loop)",
            h.frames - f0
        ));
    }
    Ok(())
}

#[test]
fn ui_live_panel_refresh_bounded() {
    check_live(Default::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_self_ui_feed_that_wakes_redraws_forever() {
    let e = check_live(forge_ui::damage::FeedFaults {
        self_ui_can_wake: true,
        ..Default::default()
    })
    .expect_err("a ui.self feed that can wake the loop must fail");
    assert!(e.contains("ui.self feed"), "{e}");
}

#[test]
fn positive_control_feed_keeping_its_waker_while_hidden_fails() {
    let e = check_live(forge_ui::damage::FeedFaults {
        keep_waker_when_hidden: true,
        ..Default::default()
    })
    .expect_err("a feed that keeps its waker while hidden must fail");
    assert!(e.contains("hidden"), "{e}");
}

// ---- ui_idle_no_busy_loop (C-ui-idle, M2-20) -------------------------------------------

/// Control-flow requests in a runner's source: `Poll` is a busy loop.
fn polls(src: &str) -> bool {
    src.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .any(|l| l.contains("ControlFlow::Poll"))
}

#[test]
fn ui_idle_no_busy_loop() {
    let (mut h, g) = harness(GalleryFaults::default());
    assert_eq!(
        h.ui.next_wake(),
        Wake::Wait,
        "settled UI must block in Wait"
    );
    // A timer: sleep exactly until its deadline, not before.
    h.ui.set_focus(Some(g.text_field), true);
    h.step();
    match h.ui.next_wake() {
        Wake::WaitUntil(t) => assert!(t > h.now(), "a deadline in the past is a busy loop"),
        other => panic!("focused field with a blinking caret: expected WaitUntil, got {other:?}"),
    }
    // The winit runner maps Wait/WaitUntil and never requests Poll.
    let runner = std::fs::read_to_string(
        forge_tests::workspace_root().join("crates/forge-ui/src/platform_winit.rs"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        runner.contains("ControlFlow::Wait"),
        "runner source not found or rewritten"
    );
    assert!(
        !polls(&runner),
        "the winit runner requests ControlFlow::Poll"
    );
}

#[test]
fn positive_control_polling_runner_fails() {
    assert!(polls("el.set_control_flow(ControlFlow::Poll);"));
}

// ---- ui_damage_bounded (C-ui-idle, M2-20) ----------------------------------------------

fn check_hover_damage(h: &mut Harness, button: WidgetId) -> Result<(), String> {
    let r = h.ui.rect(button).ok_or("no button")?;
    h.move_to(Point2::new(r.x - 30.0, r.y - 30.0));
    h.settle();
    h.move_to(r.center());
    h.step();
    let st = h.ui.stats();
    if st.last_slice_ids != vec![button] {
        return Err(format!(
            "hover re-recorded {} slices: {:?}",
            st.last_slice_ids.len(),
            st.last_slice_ids
        ));
    }
    let bound = r.outset(2.0);
    for d in &st.last_damage {
        if !bound.contains_rect(d) {
            return Err(format!("hover damage {d:?} escapes the button {bound:?}"));
        }
    }
    Ok(())
}

type Point2 = forge_ui::Point;

#[test]
fn ui_damage_bounded() {
    let (mut h, g) = harness(GalleryFaults::default());
    check_hover_damage(&mut h, g.cancel).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_hover_marking_the_root_fails() {
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.faults.hover_marks_root = true;
    let e = check_hover_damage(&mut h, g.cancel).expect_err("hover dirtying the root must fail");
    assert!(e.contains("slices"), "{e}");
}

// ---- ui_interaction_rebatches_o1 (C-ui-idle, M2-20; owner rule 2) ---------------------

/// Hovering one button of a 1,000-button panel must re-batch O(1) slices, upload O(1)
/// instances and never re-walk the paint order (the retained batch cache, ADR 0013).
fn check_interaction_o1(rebatch_everything: bool) -> Result<(u64, u32), String> {
    let mut h = Harness::new(UiConfig {
        size: forge_ui::Size::new(1920.0, 1400.0),
        ..UiConfig::default()
    })
    .map_err(|e| e.to_string())?;
    h.ui.faults.rebatch_everything = rebatch_everything;
    let panel =
        h.ui.add(
            h.ui.root(),
            "panel",
            NodeStyle::row(4.0).wrap().fill().padding(8.0),
            Container::group(),
        )
        .map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    for i in 0..1000u32 {
        ids.push(
            h.ui.add(
                panel,
                forge_ui::Key::Index(i),
                NodeStyle::leaf(),
                Button::new(format!("B{i}")),
            )
            .map_err(|e| e.to_string())?,
        );
    }
    h.settle();
    let mut worst_batched = 0u64;
    let mut worst_uploaded = 0u32;
    for id in ids.iter().step_by(97) {
        let r = h.ui.rect(*id).ok_or("no rect")?;
        h.move_to(r.center());
        let s = h.step().ok_or("hover rendered nothing")?;
        let st = h.ui.stats();
        if st.last_reassembled {
            return Err("a hover re-walked the paint order (reassembled)".into());
        }
        worst_batched = worst_batched.max(st.last_slices_batched);
        worst_uploaded = worst_uploaded.max(s.instances_uploaded);
    }
    // At most the button entered and the one left; each slot is a few instances.
    if worst_batched > 2 {
        return Err(format!(
            "a hover re-batched {worst_batched} slices (budget 2)"
        ));
    }
    if worst_uploaded > 64 {
        return Err(format!(
            "a hover uploaded {worst_uploaded} instances (budget 64)"
        ));
    }
    Ok((worst_batched, worst_uploaded))
}

#[test]
fn ui_interaction_rebatches_o1() {
    let (b, u) = check_interaction_o1(false).unwrap_or_else(|e| panic!("{e}"));
    println!(
        "ui_interaction_rebatches_o1: hover re-batched <= {b} slices, uploaded <= {u} instances"
    );
}

#[test]
fn positive_control_rebatching_everything_fails() {
    let e = check_interaction_o1(true).expect_err("re-batching every slice must fail");
    assert!(e.contains("re-walked") || e.contains("re-batched"), "{e}");
}

// ---- ui_typing_latency_one_frame (C-ui-typing-latency, M2-21) --------------------------

fn check_typing(faults: GalleryFaults) -> Result<(), String> {
    let (mut h, g) = harness(faults);
    h.ui.set_focus(Some(g.text_field), false);
    h.settle();
    let t0 = Instant::now();
    h.type_text("q");
    // Frame N: the key arrived before it.
    let stats = h.step().ok_or("the key press produced no frame")?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    if g.name.get(h.ui.rt()) != "q" {
        return Err("the model did not change in the key's frame".into());
    }
    let style = forge_ui::text::TextStyle::body(h.ui.theme().type_scale.body);
    let (run, _) = h.ui.text_mut().layout("q", &style, None);
    let in_list =
        h.ui.display_list()
            .iter()
            .any(|p| matches!(p, forge_ui::render::Primitive::Glyphs { run: r, .. } if *r == run));
    if !in_list {
        return Err("the typed text is not in frame N's display list".into());
    }
    let field = h.ui.rect(g.text_field).ok_or("no field")?.outset(2.0);
    for d in &h.ui.stats().last_damage {
        if !field.contains_rect(d) {
            return Err(format!("typing damage {d:?} escapes the field {field:?}"));
        }
    }
    println!(
        "ui_typing_latency_one_frame: key → presented frame {ms:.2} ms (debug build), {} px damaged",
        stats.damage_px
    );
    Ok(())
}

#[test]
fn ui_typing_latency_one_frame() {
    check_typing(GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_field_deferring_by_one_frame_fails() {
    let e = check_typing(GalleryFaults {
        field_defers_update: true,
        ..GalleryFaults::default()
    })
    .expect_err("a field that defers its update must fail");
    assert!(e.contains("frame"), "{e}");
}

// ---- ui_text_shaping_cached (C-ui-shaping-cache, M2-19) --------------------------------

fn check_shaping(disable_cache: bool) -> Result<(), String> {
    let mut h = Harness::new(UiConfig {
        size: forge_ui::Size::new(1600.0, 20000.0),
        ..UiConfig::default()
    })
    .map_err(|e| e.to_string())?;
    h.ui.text_mut().faults.disable_shaping_cache = disable_cache;
    let col =
        h.ui.add(
            h.ui.root(),
            "labels",
            NodeStyle::column(0.0),
            Container::group(),
        )
        .map_err(|e| e.to_string())?;
    let mut first = None;
    for i in 0..1000u32 {
        let s = h.ui.rt_mut().signal(format!("Label number {i}"));
        first.get_or_insert(s);
        h.ui.add(
            col,
            forge_ui::Key::Index(i),
            NodeStyle::leaf(),
            Label::new(s),
        )
        .map_err(|e| e.to_string())?;
    }
    h.settle();
    // Redraw all 1,000 unchanged labels (a theme switch re-lays-out and repaints all).
    let s0 = h.ui.text().stats.shape_calls;
    let t0 = Instant::now();
    h.ui.set_theme(Theme::light());
    h.step();
    let redraw_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let redraw_shapes = h.ui.text().stats.shape_calls - s0;
    if h.ui.stats().last_slices < 1000 {
        return Err(format!(
            "only {} slices re-recorded",
            h.ui.stats().last_slices
        ));
    }
    if redraw_shapes != 0 {
        return Err(format!(
            "redrawing 1,000 unchanged labels made {redraw_shapes} shape calls"
        ));
    }
    // Change one label: exactly one shape call.
    let s = first.ok_or("no labels")?;
    s.set(h.ui.rt_mut(), "Changed".into());
    let s1 = h.ui.text().stats.shape_calls;
    h.step();
    let one = h.ui.text().stats.shape_calls - s1;
    if one != 1 {
        return Err(format!("changing one label made {one} shape calls"));
    }
    println!(
        "ui_text_shaping_cached: 1,000-label full redraw {redraw_ms:.2} ms (debug), 0 shape calls"
    );
    Ok(())
}

#[test]
fn ui_text_shaping_cached() {
    check_shaping(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_shaping_cache_disabled_fails() {
    let e = check_shaping(true).expect_err("with the cache disabled redraws must shape");
    assert!(e.contains("shape calls"), "{e}");
}

// ---- ui_single_glyph_atlas (C-ui-glyph-atlas, M2-19) -----------------------------------

const EMOJI_CJK: &str = "🎨✨🚀🌍 漢字仮名交じり文 日本語のテキスト 中文字符 한국어 텍스트";

fn check_atlas(faults: forge_ui::text::TextFaults) -> Result<(), String> {
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.text_mut().faults = faults;
    h.ui.add(
        g.root,
        "emoji_cjk",
        NodeStyle::leaf(),
        Label::new(EMOJI_CJK),
    )
    .map_err(|e| e.to_string())?;
    let mut saw_colour = false;
    for theme in Theme::builtins() {
        for scale in [1.0f32, 2.0] {
            h.ui.set_theme(theme.clone());
            h.ui.set_viewport(h.ui.size(), scale);
            h.full_frame();
            if let Some(b) = &h.renderer.last {
                saw_colour |= b.instances.iter().any(|i| i.params[2] == kind::GLYPH_COLOR);
            }
        }
    }
    let t = h.ui.text();
    if t.atlas_count() != 1 {
        return Err(format!(
            "{} glyph atlases exist (budget: exactly 1)",
            t.atlas_count()
        ));
    }
    let (mask, colour) = t.atlas.plane_formats();
    if (mask, colour) != (PlaneFormat::R8Unorm, PlaneFormat::Rgba8UnormSrgb) {
        return Err(format!(
            "atlas planes are {mask:?} + {colour:?}; expected R8Unorm + Rgba8UnormSrgb"
        ));
    }
    if t.atlas.vram_cap_bytes() > 32 * 1024 * 1024 {
        return Err(format!(
            "atlas VRAM cap {} MB > 32 MB",
            t.atlas.vram_cap_bytes() / (1024 * 1024)
        ));
    }
    if !saw_colour {
        return Err(
            "no colour glyph was drawn (the emoji page did not reach the colour plane)".into(),
        );
    }
    // One bind group: the recording renderer saw exactly the three bind-group textures.
    let tex: Vec<_> = h.renderer.textures.keys().copied().collect();
    if tex.len() != 3 {
        return Err(format!("{} sampled textures created: {tex:?}", tex.len()));
    }
    println!(
        "ui_single_glyph_atlas: 1 atlas, planes {:?}, {} glyphs resident, {:.1} MB VRAM now (cap {} MB)",
        t.atlas.plane_sizes(),
        t.atlas.resident_glyphs(),
        t.atlas.vram_bytes() as f64 / (1024.0 * 1024.0),
        t.atlas.vram_cap_bytes() / (1024 * 1024)
    );
    Ok(())
}

#[test]
fn ui_single_glyph_atlas() {
    check_atlas(Default::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_one_atlas_per_font_fails() {
    let e = check_atlas(forge_ui::text::TextFaults {
        atlas_per_font: true,
        ..Default::default()
    })
    .expect_err("one atlas per font must fail");
    assert!(e.contains("glyph atlases exist"), "{e}");
}

#[test]
fn positive_control_rgba_mask_plane_fails() {
    let e = check_atlas(forge_ui::text::TextFaults {
        rgba_mask_plane: true,
        ..Default::default()
    })
    .expect_err("an RGBA8 mask plane must fail");
    assert!(e.contains("atlas planes"), "{e}");
}

/// A frame whose glyph working set exceeds the capped mask plane draws every glyph, with
/// one extra draw call per atlas epoch (§21.7).
#[test]
fn ui_single_glyph_atlas_epochs_draw_every_glyph() {
    let many: String = (0x4E00u32..0x4E00 + 600)
        .filter_map(char::from_u32)
        .collect();
    let frame = |atlas: AtlasConfig| {
        let mut h = Harness::new(UiConfig {
            atlas,
            size: forge_ui::Size::new(1600.0, 1200.0),
            ..UiConfig::default()
        })
        .unwrap_or_else(|e| panic!("{e}"));
        let big = forge_ui::text::TextStyle::body(28.0).wrapping();
        let _ = big;
        h.ui.add(
            h.ui.root(),
            "cjk",
            NodeStyle::leaf().width(1500.0),
            Label::new(many.clone()).wrapping(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let s = h.full_frame().unwrap_or_default();
        let glyphs = h
            .renderer
            .last
            .as_ref()
            .map(|b| {
                b.instances
                    .iter()
                    .filter(|i| i.params[2] == kind::GLYPH_MASK || i.params[2] == kind::GLYPH_COLOR)
                    .count()
            })
            .unwrap_or(0);
        (s, glyphs)
    };
    let (roomy, all) = frame(AtlasConfig::default());
    let tiny = AtlasConfig {
        mask_start: 128,
        mask_cap: 128,
        color_start: 64,
        color_cap: 64,
        evict_after_frames: 600,
    };
    let (tight, drawn) = frame(tiny);
    assert!(all >= 500, "only {all} glyphs in the stress page");
    assert_eq!(drawn, all, "a capped atlas dropped glyphs");
    assert!(
        tight.extra_epochs >= 1,
        "the working set fit; the test did not exercise epochs"
    );
    assert_eq!(
        tight.draw_calls,
        roomy.draw_calls + tight.extra_epochs,
        "each atlas epoch must cost exactly one extra draw call"
    );
}

// ---- ui_draw_calls_batched (C-ui-draw-batching, M2-19) ---------------------------------

fn button_panel_draw_calls(disable_batching: bool) -> (u32, f64) {
    let mut h = Harness::new(UiConfig {
        size: forge_ui::Size::new(1920.0, 1400.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    h.ui.faults.batch.disable_batching = disable_batching;
    let panel =
        h.ui.add(
            h.ui.root(),
            "panel",
            NodeStyle::row(4.0).wrap().fill().padding(8.0),
            Container::group(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    for i in 0..1000u32 {
        h.ui.add(
            panel,
            forge_ui::Key::Index(i),
            NodeStyle::leaf(),
            Button::new(format!("B{i}")),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
    h.settle();
    let t0 = Instant::now();
    let s = h.full_frame().unwrap_or_default();
    (s.draw_calls, t0.elapsed().as_secs_f64() * 1000.0)
}

#[test]
fn ui_draw_calls_batched() {
    let (calls, ms) = button_panel_draw_calls(false);
    assert!(
        calls <= 4,
        "1,000-button panel drew in {calls} draw calls (budget 4)"
    );
    println!(
        "ui_draw_calls_batched: 1,000 buttons in {calls} draw calls; batch + record {ms:.2} ms (debug)"
    );
}

#[test]
fn positive_control_batching_disabled_fails() {
    let (calls, _) = button_panel_draw_calls(true);
    assert!(
        calls > 4,
        "with batching disabled the budget still held ({calls})"
    );
}

#[test]
fn keyboard_focus_ring_does_not_break_batches() {
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.set_focus(Some(g.save), true);
    let s = h.full_frame().unwrap_or_default();
    // §21.22: a full frame draws in ≤ clip changes + viewport composites + 1. Every
    // PushClip and PopClip is a clip change; the swapchain composite is the one composite.
    let clip_changes = h
        .ui
        .display_list()
        .iter()
        .filter(|p| {
            matches!(
                p,
                forge_ui::render::Primitive::PushClip { .. } | forge_ui::render::Primitive::PopClip
            )
        })
        .count() as u32;
    let budget = clip_changes + 1 + 1;
    assert!(
        s.draw_calls <= budget,
        "gallery frame: {} draw calls ({clip_changes} clip changes; budget {budget})",
        s.draw_calls
    );
    // And the focus ring added no batch: the same frame without focus draws as many.
    h.ui.set_focus(None, false);
    let unfocused = h.full_frame().unwrap_or_default();
    assert_eq!(
        s.draw_calls, unfocused.draw_calls,
        "the focus ring broke a batch"
    );
}

#[test]
fn keys_bubble_to_global_when_field_has_nothing_to_undo() {
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.set_focus(Some(g.text_field), true);
    h.type_text("ab");
    h.key(KeyCode::Char('z'), Modifiers::CTRL);
    assert_eq!(
        g.name.get(h.ui.rt()),
        "a",
        "Ctrl+Z is field-local undo first"
    );
    h.key(KeyCode::Char('z'), Modifiers::CTRL);
    h.key(KeyCode::Char('z'), Modifiers::CTRL);
    assert_eq!(g.name.get(h.ui.rt()), "");
}

// ---- ui_virtual_{list,tree,table}_100k (C-ui-virtual-budget, M2-25) ---------------------
//
// 100,000 rows, 240 scrolled frames. Two gates:
// * live rows ≤ ⌈viewport ÷ row height⌉ + 16 on every frame (a counter: every leg);
// * layout + paint + batch + record ≤ 2.0 ms p95 (the reference machine, §21.22).
// The positive controls disable virtualisation (every row live / every cell realised)
// and, for the tree, re-splice the expanded rows one by one (an O(k) expand).
//
// Every check (gate and control) is timed alone (`forge_ui::testing::run_timed_alone`): a
// process of its own running only that test, at the High priority class. Under `cargo test`
// the thirty-odd tests of this binary are threads of one process, and neither process
// priority nor nextest's `wall-clock` group separates a gate's timed frames from its sibling
// controls' 100k-row builds. Scheduling, not a tolerance (W5).

mod virtual_budgets {
    use super::*;
    use forge_ui::InputEvent;
    use forge_ui::virtualized::update_bound;
    use forge_ui::widgets::{Column, RowItem, SortDir, TableModel, VirtualTable, VirtualTree};

    const ROWS: u64 = 100_000;
    const VIEW_H: f32 = 600.0;
    const ROW_H: f32 = 24.0;
    const FRAMES: usize = 240;
    const BUDGET_MS: f64 = 2.0;

    fn live_bound() -> usize {
        (VIEW_H / ROW_H).ceil() as usize + 16
    }

    fn p95(mut v: Vec<f64>) -> f64 {
        v.sort_by(f64::total_cmp);
        v[((v.len() as f64 * 0.95).ceil() as usize).min(v.len()) - 1]
    }

    /// The host every timed check builds first (inside `run_timed_alone`).
    fn host(h: &mut Harness) -> WidgetId {
        h.ui.add(
            h.ui.root(),
            "host",
            NodeStyle::column(0.0).fill().padding(8.0),
            Container::group(),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    /// Scroll `id` with the wheel for `FRAMES` frames (alternating page-sized and
    /// row-sized steps, reversing direction every 60 frames), timing each frame. Returns
    /// (p95 ms, the largest count `live` reported).
    fn scroll_and_time(
        h: &mut Harness,
        id: WidgetId,
        live: &dyn Fn(&Harness) -> usize,
    ) -> (f64, usize) {
        let r = h.ui.rect(id).unwrap_or_default();
        let at = r.center();
        h.input(InputEvent::PointerMoved(at));
        h.settle();
        let mut times = Vec::with_capacity(FRAMES);
        let mut max_live = live(h);
        for f in 0..FRAMES {
            let dy = match ((f / 60) % 2, f % 2) {
                (0, 0) => -8.0,
                (0, _) => -1.0,
                (_, 0) => 8.0,
                _ => 1.0,
            };
            h.input(InputEvent::Wheel {
                pos: at,
                dx: 0.0,
                dy,
            });
            let t0 = Instant::now();
            h.step();
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            max_live = max_live.max(live(h));
            if max_live > live_bound() {
                break; // the counter gate already failed; do not time 240 broken frames
            }
        }
        (p95(times), max_live)
    }

    fn list_harness(fault: bool) -> (Harness, WidgetId) {
        let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
        let host = host(&mut h);
        let mut v = VirtualTree::list("Entities").row_height(ROW_H);
        if fault {
            v = v.with_fault_no_virtualisation();
        }
        v.extend((0..ROWS).map(|i| (i, RowItem::new(format!("Entity {i}")))));
        let id =
            h.ui.add(host, "list", NodeStyle::leaf().size(420.0, VIEW_H), v)
                .unwrap_or_else(|e| panic!("{e}"));
        h.settle();
        (h, id)
    }

    fn tree_live(h: &Harness, id: WidgetId) -> usize {
        h.ui.widget::<VirtualTree>(id).map_or(0, |t| t.live_rows())
    }

    fn check_list(fault: bool) -> Result<String, String> {
        forge_ui::testing::run_timed_alone(|| {
            let (mut h, id) = list_harness(fault);
            let (p95, live) = scroll_and_time(&mut h, id, &|h| tree_live(h, id));
            if live > live_bound() {
                return Err(format!("list: {live} live rows (bound {})", live_bound()));
            }
            if p95 > BUDGET_MS {
                return Err(format!(
                    "list: p95 {p95:.3} ms over {FRAMES} frames (budget {BUDGET_MS} ms)"
                ));
            }
            Ok(format!(
                "p95 {p95:.3} ms layout+paint, {live} live rows (bound {})",
                live_bound()
            ))
        })
    }

    #[test]
    fn ui_virtual_list_100k() {
        let s = check_list(false).unwrap_or_else(|e| panic!("{e}"));
        println!("ui_virtual_list_100k: {s}");
    }

    #[test]
    fn positive_control_list_without_virtualisation_fails() {
        let e = check_list(true).err().unwrap_or_default();
        assert!(
            e.contains("live rows"),
            "the budget was broken but the infrastructure returned an Ok: {e}"
        );
    }

    const BIG: u64 = 1 << 40;

    /// A 100k-node tree: one 50,000-child folder ("Big") plus 500 folders of 100.
    fn tree_harness(fault_splice: bool) -> (Harness, WidgetId) {
        let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
        let host = host(&mut h);
        let mut t = VirtualTree::tree("Scene").row_height(ROW_H);
        t.push(None, BIG, RowItem::new("Big"));
        for i in 0..50_000u64 {
            t.push(Some(BIG), BIG + 1 + i, RowItem::new(format!("Leaf {i}")));
        }
        for f in 0..500u64 {
            let fk = (1u64 << 41) | (f << 20);
            t.push(None, fk, RowItem::new(format!("Folder {f}")));
            for c in 0..100u64 {
                t.push(Some(fk), fk + 1 + c, RowItem::new(format!("Item {f}.{c}")));
            }
            t.set_expanded(fk, true);
        }
        t.set_fault_splice_rows(fault_splice);
        let id =
            h.ui.add(host, "tree", NodeStyle::leaf().size(420.0, VIEW_H), t)
                .unwrap_or_else(|e| panic!("{e}"));
        h.settle();
        (h, id)
    }

    fn check_tree(fault_splice: bool) -> Result<String, String> {
        forge_ui::testing::run_timed_alone(|| measure_tree(fault_splice))
    }

    fn measure_tree(fault_splice: bool) -> Result<String, String> {
        let (mut h, id) = tree_harness(fault_splice);
        let (rows, (d, b)) =
            h.ui.widget::<VirtualTree>(id)
                .map_or((0, (0, 0)), |t| (t.row_count(), t.index().shape()));
        if rows != 500 * 101 + 1 {
            return Err(format!("tree shows {rows} rows before expanding Big"));
        }
        // Expand and collapse the 50k subtree: bounded index updates and ≤ 2 ms.
        let bound = update_bound(d, b);
        let mut worst = 0.0f64;
        for on in [true, false, true] {
            let t0 = Instant::now();
            let updates =
                VirtualTree::edit(&mut h.ui, id, |t| t.set_expanded(BIG, on)).unwrap_or(u64::MAX);
            h.step();
            worst = worst.max(t0.elapsed().as_secs_f64() * 1000.0);
            if updates > bound {
                return Err(format!(
                    "expand({on}) of a 50k subtree updated {updates} index nodes (bound {bound} for d={d}, b={b})"
                ));
            }
        }
        let rows = h.ui.widget::<VirtualTree>(id).map_or(0, |t| t.row_count());
        if rows != 500 * 101 + 1 + 50_000 {
            return Err(format!("{rows} rows after expanding Big"));
        }
        if worst > BUDGET_MS {
            return Err(format!(
                "expand/collapse of 50k rows took {worst:.3} ms (budget {BUDGET_MS} ms)"
            ));
        }
        let (p95, live) = scroll_and_time(&mut h, id, &|h| tree_live(h, id));
        if live > live_bound() {
            return Err(format!("tree: {live} live rows (bound {})", live_bound()));
        }
        if p95 > BUDGET_MS {
            return Err(format!("tree: p95 {p95:.3} ms (budget {BUDGET_MS} ms)"));
        }
        Ok(format!(
            "p95 {p95:.3} ms scrolling, expand/collapse 50k <= {worst:.3} ms, {live} live rows, index updates <= {bound}"
        ))
    }

    #[test]
    fn ui_virtual_tree_100k() {
        let s = check_tree(false).unwrap_or_else(|e| panic!("{e}"));
        println!("ui_virtual_tree_100k: {s}");
    }

    #[test]
    fn positive_control_tree_row_by_row_splice_fails() {
        let e = check_tree(true).err().unwrap_or_default();
        assert!(e.contains("index nodes"), "an O(k) splice passed: {e:?}");
    }

    /// 100k rows x 8 columns; sorting keeps a permutation (model side, §21.12).
    struct Model {
        order: Vec<u32>,
    }
    fn stat(i: u32, col: usize) -> u64 {
        (u64::from(i).wrapping_mul(2_654_435_761) >> (col * 3)) % 10_000
    }
    impl TableModel for Model {
        fn len(&self) -> usize {
            self.order.len()
        }
        fn key(&self, row: usize) -> u64 {
            u64::from(self.order[row])
        }
        fn cell(&self, row: usize, col: usize) -> String {
            let i = self.order[row];
            match col {
                0 => format!("Row {i}"),
                c => stat(i, c).to_string(),
            }
        }
        fn sort(&mut self, col: usize, dir: SortDir) {
            if col == 0 {
                self.order.sort_unstable();
            } else {
                self.order.sort_by_key(|i| (stat(*i, col), *i));
            }
            if dir == SortDir::Descending {
                self.order.reverse();
            }
        }
    }

    fn check_table(fault: bool) -> Result<String, String> {
        forge_ui::testing::run_timed_alone(|| measure_table(fault))
    }

    fn measure_table(fault: bool) -> Result<String, String> {
        let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
        let host = host(&mut h);
        let cols = (0..8)
            .map(|c| {
                if c == 0 {
                    Column::new("Name", 120.0)
                } else {
                    Column::new(&format!("C{c}"), 80.0).numeric()
                }
            })
            .collect();
        let mut t = VirtualTable::new(
            "Stats",
            cols,
            Box::new(Model {
                order: (0..ROWS as u32).collect(),
            }),
        );
        if fault {
            t = t.with_fault_realise_all_rows();
        }
        let id =
            h.ui.add(host, "table", NodeStyle::leaf().size(760.0, VIEW_H), t)
                .unwrap_or_else(|e| panic!("{e}"));
        h.settle();
        // Sort and resize, as a user would, then scroll.
        h.ui.set_focus(Some(id), true);
        h.key(KeyCode::Right, Modifiers::NONE);
        h.key(KeyCode::Enter, Modifiers::CTRL);
        h.key(
            KeyCode::Right,
            Modifiers {
                alt: true,
                ..Modifiers::NONE
            },
        );
        h.settle();
        let (sorted, width) =
            h.ui.widget::<VirtualTable>(id)
                .map_or((None, 0.0), |t| (t.sort_state(), t.columns()[1].width));
        if sorted != Some((1, SortDir::Ascending)) || width != 88.0 {
            return Err(format!("sort {sorted:?}, width {width}"));
        }
        let cells = |h: &Harness| {
            h.ui.widget::<VirtualTable>(id)
                .map_or(0, |t| t.cells_recorded() / 8)
        };
        let (p95, rows_realised) = scroll_and_time(&mut h, id, &cells);
        let live = h.ui.widget::<VirtualTable>(id).map_or(0, |t| t.live_rows());
        if rows_realised > live_bound() || live > live_bound() {
            return Err(format!(
                "table: {rows_realised} rows of cells realised, {live} live (bound {})",
                live_bound()
            ));
        }
        if p95 > BUDGET_MS {
            return Err(format!("table: p95 {p95:.3} ms (budget {BUDGET_MS} ms)"));
        }
        Ok(format!("p95 {p95:.3} ms, {live} live rows x 8 columns"))
    }

    #[test]
    fn ui_virtual_table_100k() {
        let s = check_table(false).unwrap_or_else(|e| panic!("{e}"));
        println!("ui_virtual_table_100k: {s}");
    }

    #[test]
    fn positive_control_table_realising_offscreen_rows_fails() {
        let e = check_table(true).err().unwrap_or_default();
        assert!(
            e.contains("live") || e.contains("budget"),
            "the budget was broken but the infrastructure returned an Ok: {e}"
        );
    }
}

// ---- ui_graph_2k_nodes (C-ui-graph-budget, M2-46, WP-U8) ---------------------------------
//
// A 2,000-node graph (≈3,900 wires) on the node canvas in a 1440×900 window, 240 frames of
// a middle-button pan with a wheel zoom every 12th frame (zoom swinging between ≈0.5 and
// ≈2.0). Two gates:
// * every frame records exactly the nodes that intersect the view (a counter, every leg),
//   and the minimap — the one slice holding every node — is not re-recorded by a pan or a
//   zoom (only its one-quad view frame is);
// * pan/zoom frame (event + layout + paint + batch + record) ≤ 4.0 ms p95 (§21.22).
// The positive control records every node on every frame.

mod graph_budget {
    use super::*;
    use forge_ui::style::ColorRole;
    use forge_ui::widgets::{CanvasNode, CanvasPin, CanvasView, CanvasWire, NodeCanvas, PinRef};
    use forge_ui::{InputEvent, Point, PointerButton, Size};

    const NODES: u64 = 2_000;
    const COLS: u64 = 50;
    const FRAMES: usize = 240;
    const BUDGET_MS: f64 = 4.0;
    const TITLES: [&str; 8] = [
        "Kinetic energy",
        "Multiply",
        "Add",
        "Momentum",
        "Lerp",
        "Noise",
        "Clamp",
        "Distance",
    ];

    fn p95(mut v: Vec<f64>) -> f64 {
        v.sort_by(f64::total_cmp);
        v[((v.len() as f64 * 0.95).ceil() as usize).min(v.len()) - 1]
    }

    fn graph(record_every_node: bool) -> (Harness, WidgetId) {
        let mut h = Harness::new(UiConfig {
            size: Size::new(1440.0, 900.0),
            ..UiConfig::default()
        })
        .unwrap_or_else(|e| panic!("{e}"));
        let root = h.ui.root();
        let canvas = NodeCanvas::build_with(
            &mut h.ui,
            root,
            "graph",
            NodeStyle::default().fill(),
            "Graph",
            record_every_node,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        h.settle();
        let pin = |l: &str| CanvasPin::new(l, "f64 · m/s", ColorRole::Accent);
        NodeCanvas::edit(&mut h.ui, canvas, |e| {
            for k in 0..NODES {
                let pos = Point::new((k % COLS) as f32 * 240.0, (k / COLS) as f32 * 150.0);
                e.set_node(
                    k,
                    CanvasNode::new(TITLES[(k % 8) as usize], pos)
                        .with_inputs(vec![pin("a"), pin("b")])
                        .with_outputs(vec![pin("return")]),
                );
            }
            for k in 0..NODES {
                if k % COLS != 0 {
                    e.set_wire(CanvasWire {
                        from: PinRef::output(k - 1, 0),
                        to: PinRef::input(k, 0),
                        role: ColorRole::Accent,
                    });
                }
                if k >= COLS {
                    e.set_wire(CanvasWire {
                        from: PinRef::output(k - COLS, 0),
                        to: PinRef::input(k, 1),
                        role: ColorRole::Success,
                    });
                }
            }
            e.set_view(CanvasView {
                pan: Point::new(2000.0, 1500.0),
                zoom: 1.0,
            });
        });
        h.settle();
        (h, canvas)
    }

    /// Timed alone (`run_timed_alone`): its own process, High priority class (W5).
    fn check(record_every_node: bool) -> Result<String, String> {
        forge_ui::testing::run_timed_alone(|| measure(record_every_node))
    }

    fn measure(record_every_node: bool) -> Result<String, String> {
        let (mut h, id) = graph(record_every_node);
        let stats = |h: &Harness| {
            h.ui.widget::<NodeCanvas>(id)
                .map(|c| (c.stats(), c.model().visible_nodes(), c.model().view()))
                .unwrap_or_default()
        };
        let r = h.ui.rect(id).unwrap_or_default();
        let mut at = r.center();
        h.input(InputEvent::PointerMoved(at));
        h.input(InputEvent::PointerButton {
            pos: at,
            button: PointerButton::Middle,
            pressed: true,
        });
        h.settle();
        let (s0, _, _) = stats(&h);
        let mut times = Vec::with_capacity(FRAMES);
        let (mut most, mut zmin, mut zmax) = (0usize, f32::MAX, 0.0f32);
        for f in 0..FRAMES {
            let t0 = Instant::now();
            if f % 12 == 11 {
                // In five steps (to ≈2.0), out ten (to ≈0.5), in five (back to ≈1.0).
                let step = f / 12;
                let dy = if (5..15).contains(&step) { -1.5 } else { 1.5 };
                h.input(InputEvent::Wheel {
                    pos: at,
                    dx: 0.0,
                    dy,
                });
            } else {
                // The pointer swings back and forth inside the window (it drags the view).
                let dx = if (f / 20) % 2 == 0 { -23.0 } else { 23.0 };
                let dy = if (f / 10) % 2 == 0 { -9.0 } else { 9.0 };
                at = Point::new(at.x + dx, at.y + dy);
                h.input(InputEvent::PointerMoved(at));
            }
            h.step();
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            let (s, visible, view) = stats(&h);
            zmin = zmin.min(view.zoom);
            zmax = zmax.max(view.zoom);
            most = most.max(s.nodes_recorded);
            if s.nodes_recorded > visible {
                return Err(format!(
                    "a pan/zoom frame re-recorded {} nodes with {visible} in view (frame {f})",
                    s.nodes_recorded
                ));
            }
        }
        let (s1, _, _) = stats(&h);
        if s1.minimap_paints != s0.minimap_paints {
            return Err(format!(
                "the minimap (every node) was re-recorded {} times by pans and zooms",
                s1.minimap_paints - s0.minimap_paints
            ));
        }
        if s1.frame_paints < s0.frame_paints + FRAMES as u64 / 2 {
            return Err("the minimap's view frame did not follow the pan".into());
        }
        if zmin > 0.6 || zmax < 1.8 {
            return Err(format!(
                "the walk did not zoom out and in ({zmin:.2}..{zmax:.2})"
            ));
        }
        let p = p95(times);
        if p > BUDGET_MS {
            return Err(format!(
                "graph: pan/zoom p95 {p:.3} ms over {FRAMES} frames (budget {BUDGET_MS} ms)"
            ));
        }
        Ok(format!(
            "p95 {p:.3} ms per pan/zoom frame, at most {most} of {NODES} nodes recorded, zoom {zmin:.2}..{zmax:.2}"
        ))
    }

    #[test]
    fn ui_graph_2k_nodes() {
        let s = check(false).unwrap_or_else(|e| panic!("{e}"));
        println!("ui_graph_2k_nodes: {s}");
    }

    #[test]
    fn positive_control_graph_recording_every_node_fails() {
        let e = check(true).err().unwrap_or_default();
        assert!(
            e.contains("re-recorded"),
            "a canvas recording every node per pan frame passed: {e:?}"
        );
    }
}
