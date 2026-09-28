//! Catalogue widget tests, part 1 (§21.16; DoD M2-24): text & display, buttons &
//! toggles, numeric. Each widget: its AccessKit role and name, and its keyboard (and
//! pointer) input.

use forge_ui::testing::Harness;
use forge_ui::widgets::*;
use forge_ui::{KeyCode, Modifiers, NodeStyle, Role, WidgetId};

fn one(w: impl forge_ui::Widget) -> (Harness, WidgetId) {
    Harness::with_widget(w, NodeStyle::leaf()).unwrap_or_else(|e| panic!("{e}"))
}
fn sized(w: impl forge_ui::Widget, width: f32) -> (Harness, WidgetId) {
    Harness::with_widget(w, NodeStyle::leaf().width(width)).unwrap_or_else(|e| panic!("{e}"))
}
fn label_of(h: &mut Harness, id: WidgetId) -> String {
    h.node(id)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default()
}
const NONE: Modifiers = Modifiers::NONE;

// ---- text & display ----------------------------------------------------------------------

#[test]
fn label_names_itself_and_follows_its_binding() {
    let (mut h, id) = one(Label::new("Mass"));
    assert_eq!(h.ui.role(id), Some(Role::Label));
    assert_eq!(label_of(&mut h, id), "Mass");
    assert!(!h.ui.is_focusable(id), "a label is not a tab stop");
}

#[test]
fn bound_label_reshapes_on_change() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let s = h.ui.rt_mut().signal(String::from("one"));
    let row =
        h.ui.add(host, "row", NodeStyle::row(4.0), Container::group())
            .unwrap_or_else(|e| panic!("{e}"));
    let id =
        h.ui.add(row, "l", NodeStyle::leaf(), Label::new(s))
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let w0 = h.ui.rect(id).unwrap_or_default().w;
    s.set(h.ui.rt_mut(), String::from("a much longer label"));
    h.settle();
    assert!(h.ui.rect(id).unwrap_or_default().w > w0);
    assert_eq!(label_of(&mut h, id), "a much longer label");
}

#[test]
fn rich_text_links_are_keyboard_operable() {
    let (mut h, id) = sized(
        RichText::new(vec![
            Span::plain("See "),
            Span::link("the docs", "forge://docs"),
            Span::plain(" or "),
            Span::link("help", "forge://help"),
        ]),
        400.0,
    );
    assert_eq!(h.ui.role(id), Some(Role::Paragraph));
    assert!(h.ui.is_focusable(id));
    h.focus(id);
    h.take::<LinkActivated>();
    h.press(KeyCode::Enter, NONE);
    h.press(KeyCode::Right, NONE);
    h.press(KeyCode::Enter, NONE);
    let got: Vec<String> = h
        .take::<LinkActivated>()
        .into_iter()
        .map(|l| l.target)
        .collect();
    assert_eq!(
        got.last().map(String::as_str),
        Some("forge://help"),
        "{got:?}"
    );
    let links = (0..4)
        .filter_map(|i| h.child_node(id, i))
        .filter(|n| n.role() == Role::Link)
        .count();
    assert!(links >= 2, "each link is an AccessKit link ({links})");
}

#[test]
fn display_widgets_have_roles_and_names() {
    let (mut h, id) = one(Icon::new("⚙", "Settings"));
    assert_eq!(h.ui.role(id), Some(Role::Image));
    assert_eq!(label_of(&mut h, id), "Settings");
    let (mut h, id) = one(Badge::new("12"));
    assert_eq!(h.ui.role(id), Some(Role::Status));
    assert!(label_of(&mut h, id).contains("12"));
    let (mut h, id) = one(ShortcutHint::new("Ctrl+S"));
    assert_eq!(h.ui.role(id), Some(Role::Label));
    assert!(label_of(&mut h, id).contains("Ctrl"));
    let (h, id) = Harness::with_widget(Separator::horizontal(), NodeStyle::leaf().width(100.0))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(h.ui.role(id), Some(Role::Splitter));
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let img =
        h.ui.add_image(2, 2, vec![255; 16])
            .unwrap_or_else(|e| panic!("{e}"));
    let id =
        h.ui.add(
            host,
            "i",
            NodeStyle::leaf(),
            ImageView::new(img, forge_ui::Size::new(8.0, 8.0), "Thumbnail"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(id), Some(Role::Image));
    assert_eq!(label_of(&mut h, id), "Thumbnail");
}

// ---- buttons & toggles ---------------------------------------------------------------------

#[test]
fn button_variants_and_sizes_press_by_keyboard_and_pointer() {
    for v in forge_ui::style::Variant::ALL {
        let (mut h, id) = one(Button::new("Go").variant(v));
        assert_eq!(h.ui.role(id), Some(Role::Button));
        assert_eq!(label_of(&mut h, id), "Go");
        h.focus(id);
        h.press(KeyCode::Enter, NONE);
        h.press(KeyCode::Space, NONE);
        h.click(id);
        h.settle();
        assert_eq!(
            h.take::<Pressed>().len(),
            3,
            "{v:?}: Enter, Space and a click each press"
        );
    }
    let (hs, s) = one(Button::new("Go").size(ButtonSize::Small));
    let (hl, l) = one(Button::new("Go").size(ButtonSize::Large));
    assert!(hs.ui.rect(s).unwrap_or_default().h < hl.ui.rect(l).unwrap_or_default().h);
}

#[test]
fn icon_button_is_named_and_has_a_tooltip() {
    let (mut h, id) = one(IconButton::new("✎", "Edit"));
    assert_eq!(h.ui.role(id), Some(Role::Button));
    assert_eq!(label_of(&mut h, id), "Edit");
    h.focus(id);
    h.advance(forge_ui::overlay::TOOLTIP_DELAY + std::time::Duration::from_millis(10));
    assert!(
        h.ui.tooltip_shown().is_some(),
        "keyboard focus shows the tooltip"
    );
    h.press(KeyCode::Enter, NONE);
    assert_eq!(h.take::<Pressed>().len(), 1);
}

#[test]
fn toggles_flip_with_space_and_report_state() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let a = h.ui.rt_mut().signal(false);
    let b = h.ui.rt_mut().signal(false);
    let c = h.ui.rt_mut().signal(false);
    let t =
        h.ui.add(host, "t", NodeStyle::leaf(), ToggleButton::new(a, "Snap"))
            .unwrap_or_else(|e| panic!("{e}"));
    let s =
        h.ui.add(host, "s", NodeStyle::leaf(), Switch::new(b, "Autosave"))
            .unwrap_or_else(|e| panic!("{e}"));
    let k =
        h.ui.add(host, "k", NodeStyle::leaf(), Checkbox::new(c, "Shadows"))
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    for (id, sig, role) in [
        (t, a, Role::Button),
        (s, b, Role::Switch),
        (k, c, Role::CheckBox),
    ] {
        assert_eq!(h.ui.role(id), Some(role));
        h.focus(id);
        h.press(KeyCode::Space, NONE);
        assert!(sig.get(h.ui.rt()), "{role:?} toggled on");
        assert_eq!(
            h.node(id).and_then(|n| n.toggled()),
            Some(accesskit::Toggled::True)
        );
        h.click(id);
        h.settle();
        assert!(!sig.get(h.ui.rt()), "{role:?} toggled off by a click");
    }
}

#[test]
fn tri_state_checkbox_resolves_mixed() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let s = h.ui.rt_mut().signal(CheckState::Mixed);
    let id =
        h.ui.add(
            host,
            "c",
            NodeStyle::leaf(),
            TriCheckbox::new(s, "All layers"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(
        h.node(id).and_then(|n| n.toggled()),
        Some(accesskit::Toggled::Mixed)
    );
    h.focus(id);
    h.press(KeyCode::Space, NONE);
    assert_ne!(
        s.get(h.ui.rt()),
        CheckState::Mixed,
        "Space resolves a mixed state"
    );
}

#[test]
fn radio_group_and_segmented_control_move_with_arrows() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let r = h.ui.rt_mut().signal(0usize);
    let g = h.ui.rt_mut().signal(0usize);
    let rid =
        h.ui.add(
            host,
            "r",
            NodeStyle::leaf(),
            RadioGroup::new("Units", &["Metric", "Imperial"], r),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let gid =
        h.ui.add(
            host,
            "g",
            NodeStyle::leaf(),
            SegmentedControl::new("View", &["2D", "3D", "Split"], g),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(rid), Some(Role::RadioGroup));
    assert_eq!(h.ui.role(gid), Some(Role::RadioGroup));
    h.focus(rid);
    h.press(KeyCode::Down, NONE);
    assert_eq!(r.get(h.ui.rt()), 1);
    h.focus(gid);
    h.press(KeyCode::End, NONE);
    assert_eq!(g.get(h.ui.rt()), 2);
    h.press(KeyCode::Right, NONE);
    assert_eq!(g.get(h.ui.rt()), 0, "wraps");
    assert_eq!(
        h.node(gid)
            .and_then(|n| n.value().map(str::to_string))
            .as_deref(),
        Some("2D")
    );
}

// ---- numeric -----------------------------------------------------------------------------

#[test]
fn slider_steps_jumps_and_maps_logarithmically() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let lin = h.ui.rt_mut().signal(0.5f32);
    let log = h.ui.rt_mut().signal(1.0f32);
    let a =
        h.ui.add(
            host,
            "a",
            NodeStyle::leaf().width(200.0),
            Slider::new(lin, "Roughness", 0.0, 1.0, 0.1),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let b =
        h.ui.add(
            host,
            "b",
            NodeStyle::leaf().width(200.0),
            Slider::new(log, "Distance", 0.01, 100.0, 0.01).log(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(a), Some(Role::Slider));
    h.focus(a);
    h.press(KeyCode::Right, NONE);
    assert!((lin.get(h.ui.rt()) - 0.6).abs() < 1e-6);
    h.press(KeyCode::Home, NONE);
    assert_eq!(lin.get(h.ui.rt()), 0.0);
    assert_eq!(h.node(a).and_then(|n| n.numeric_value()), Some(0.0));
    // Log: the midpoint of the track is the geometric mean (1.0 for 0.01..100).
    let r = h.ui.rect(b).unwrap_or_default();
    h.click_at(forge_ui::Point::new(r.center().x, r.center().y));
    let v = log.get(h.ui.rt());
    assert!((0.5..2.0).contains(&v), "log midpoint {v}");
}

#[test]
fn numeric_field_converts_units_and_refuses_incompatible_ones() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let v = h.ui.rt_mut().signal(12.5f64);
    let id =
        h.ui.add(
            host,
            "n",
            NodeStyle::leaf().width(160.0),
            NumericField::new(v, "Impulse")
                .unit("N·s")
                .step(1.0)
                .decimals(2),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(id), Some(Role::SpinButton));
    assert_eq!(label_of(&mut h, id), "Impulse");
    h.focus(id);
    h.press(KeyCode::Up, NONE);
    assert!(
        (v.get(h.ui.rt()) - 13.5).abs() < 1e-9,
        "{}",
        v.get(h.ui.rt())
    );
    h.take::<NumericCommitted>();
    // Type a compatible unit: converted on entry.
    h.press(KeyCode::Enter, NONE);
    h.press(KeyCode::Char('a'), Modifiers::CTRL);
    h.type_text("2 kN·s");
    h.press(KeyCode::Enter, NONE);
    assert!(
        (v.get(h.ui.rt()) - 2000.0).abs() < 1e-9,
        "{}",
        v.get(h.ui.rt())
    );
    assert_eq!(h.take::<NumericCommitted>().len(), 1);
    // An incompatible unit is refused with a reason; the value is unchanged.
    h.press(KeyCode::Enter, NONE);
    h.press(KeyCode::Char('a'), Modifiers::CTRL);
    h.type_text("3 kg");
    h.press(KeyCode::Enter, NONE);
    assert!((v.get(h.ui.rt()) - 2000.0).abs() < 1e-9);
    let err =
        h.ui.widget::<NumericField>(id)
            .and_then(|f| f.error().map(str::to_string))
            .unwrap_or_default();
    assert!(
        err.contains("kg") || err.to_lowercase().contains("mass"),
        "reason names the problem: {err:?}"
    );
}

/// Dragging a numeric field scrubs its value live and ends in exactly one
/// `NumericCommitted` (§21.18: one transaction per gesture); a press that never moves past
/// the 3 px slop opens the text editor and commits nothing.
#[test]
fn numeric_field_drag_scrubs_live_and_commits_once_per_gesture() {
    use forge_ui::Point;
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let v = h.ui.rt_mut().signal(10.0f64);
    let id =
        h.ui.add(
            host,
            "n",
            NodeStyle::leaf().width(160.0),
            NumericField::new(v, "Mass")
                .unit("kg")
                .step(0.5)
                .range(0.0, 100.0),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let r = h.ui.rect(id).unwrap_or_else(|| panic!("laid out"));
    let a = Point::new(r.x + 20.0, r.center().y);
    h.take::<NumericCommitted>();

    // Press, then move in steps: the value follows the pointer while the button is down,
    // but nothing is committed until release.
    h.ui.handle(forge_ui::InputEvent::PointerMoved(a));
    h.ui.handle(forge_ui::InputEvent::PointerButton {
        pos: a,
        button: forge_ui::PointerButton::Primary,
        pressed: true,
    });
    for dx in [2.0f32, 6.0, 12.0, 20.0] {
        h.ui.handle(forge_ui::InputEvent::PointerMoved(Point::new(
            a.x + dx,
            a.y,
        )));
        h.settle();
        if dx < 3.0 {
            assert_eq!(v.get(h.ui.rt()), 10.0, "inside the slop nothing moves");
        } else {
            let want = 10.0 + f64::from(dx) * 0.5;
            assert!(
                (v.get(h.ui.rt()) - want).abs() < 1e-9,
                "live scrub at dx={dx}: {} != {want}",
                v.get(h.ui.rt())
            );
        }
        assert!(
            h.take::<NumericCommitted>().is_empty(),
            "no commit mid-gesture"
        );
    }
    let end = Point::new(a.x + 20.0, a.y);
    h.ui.handle(forge_ui::InputEvent::PointerButton {
        pos: end,
        button: forge_ui::PointerButton::Primary,
        pressed: false,
    });
    h.settle();
    let commits = h.take::<NumericCommitted>();
    assert_eq!(commits.len(), 1, "exactly one commit per drag: {commits:?}");
    assert_eq!(commits[0].field, id);
    assert!((commits[0].value - 20.0).abs() < 1e-9, "{:?}", commits[0]);
    assert!(
        !h.ui
            .widget::<NumericField>(id)
            .is_some_and(|f| f.is_editing()),
        "a drag does not open the editor"
    );

    // The drag range clamps, still in one commit.
    h.drag(end, Point::new(end.x + 400.0, end.y), 10);
    assert_eq!(v.get(h.ui.rt()), 100.0, "clamped to the range");
    assert_eq!(h.take::<NumericCommitted>().len(), 1);

    // A press that stays inside the slop is a click: it opens the editor, commits nothing.
    h.drag(end, Point::new(end.x + 1.0, end.y), 1);
    assert_eq!(v.get(h.ui.rt()), 100.0);
    assert!(h.take::<NumericCommitted>().is_empty());
    assert!(
        h.ui.widget::<NumericField>(id)
            .is_some_and(|f| f.is_editing()),
        "a click opens the text editor"
    );
}

#[test]
fn spin_box_steps_within_its_range() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let v = h.ui.rt_mut().signal(63.0f64);
    let id =
        h.ui.add(
            host,
            "n",
            NodeStyle::leaf().width(120.0),
            NumericField::new(v, "Samples")
                .range(1.0, 64.0)
                .step(1.0)
                .decimals(0)
                .spin(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    h.focus(id);
    h.press(KeyCode::Up, NONE);
    h.press(KeyCode::Up, NONE);
    assert_eq!(v.get(h.ui.rt()), 64.0, "clamped to the range");
    h.press(KeyCode::Home, NONE);
    assert_eq!(v.get(h.ui.rt()), 1.0);
}

#[test]
fn range_slider_thumbs_are_separate_stops_and_never_cross() {
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let v = h.ui.rt_mut().signal((20.0f32, 30.0f32));
    let id =
        h.ui.add(
            host,
            "r",
            NodeStyle::leaf().width(200.0),
            RangeSlider::new(v, "LOD", 0.0, 100.0, 5.0),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    h.focus(id);
    for _ in 0..5 {
        h.press(KeyCode::Right, NONE);
    }
    let (lo, hi) = v.get(h.ui.rt());
    assert!(lo <= hi, "thumbs crossed: {lo} > {hi}");
    h.tab(false);
    h.settle();
    assert_eq!(
        h.ui.widget::<RangeSlider>(id).map(|r| r.active_thumb()),
        Some(1),
        "Tab moves to the high thumb"
    );
    h.press(KeyCode::End, NONE);
    assert_eq!(v.get(h.ui.rt()).1, 100.0);
}
