//! Catalogue widget tests, part 4 (§21.16; DoD M2-24): containers, menus & overlays,
//! feedback, navigation.

use std::rc::Rc;
use std::time::Duration;

use forge_ui::overlay::{Severity, TOAST_DURATION};
use forge_ui::testing::Harness;
use forge_ui::widgets::*;
use forge_ui::{Key, KeyCode, Modifiers, NodeStyle, Role, Size, WidgetId};

const NONE: Modifiers = Modifiers::NONE;

fn host() -> (Harness, WidgetId) {
    Harness::with_host().unwrap_or_else(|e| panic!("{e}"))
}
fn add(
    h: &mut Harness,
    host: WidgetId,
    key: &'static str,
    style: NodeStyle,
    w: impl forge_ui::Widget,
) -> WidgetId {
    let id =
        h.ui.add(host, key, style, w)
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    id
}
fn label_of(h: &mut Harness, id: WidgetId) -> String {
    h.node(id)
        .and_then(|n| n.label().map(str::to_string))
        .unwrap_or_default()
}

// ---- containers --------------------------------------------------------------------------

#[test]
fn scroll_area_scrolls_by_keyboard_and_follows_focus() {
    let (mut h, host) = host();
    let sa = scroll_area(
        &mut h.ui,
        host,
        "sa",
        NodeStyle::default().size(200.0, 100.0),
        "Notes",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut last = sa.area;
    for i in 0..20u32 {
        last =
            h.ui.add(
                sa.viewport,
                Key::Index(i),
                NodeStyle::leaf(),
                Button::new(format!("Item {i}")),
            )
            .unwrap_or_else(|e| panic!("{e}"));
    }
    h.settle();
    assert_eq!(h.ui.role(sa.area), Some(Role::ScrollView));
    assert_eq!(h.ui.role(sa.scrollbar), Some(Role::ScrollBar));
    h.focus(sa.area);
    h.press(KeyCode::PageDown, NONE);
    let y = h.ui.scroll_state(sa.viewport).map_or(0.0, |s| s.offset.y);
    assert!(y > 0.0, "PageDown scrolled ({y})");
    h.press(KeyCode::Home, NONE);
    assert_eq!(
        h.ui.scroll_state(sa.viewport).map(|s| s.offset.y),
        Some(0.0)
    );
    // Keyboard focus onto a widget scrolled out of view brings it into view.
    h.ui.set_focus(Some(last), true);
    h.settle();
    let vr = h.ui.rect(sa.viewport).unwrap_or_default();
    let lr = h.ui.rect(last).unwrap_or_default();
    assert!(
        lr.bottom() <= vr.bottom() + 0.5 && lr.y >= vr.y - 0.5,
        "{lr:?} not inside {vr:?}"
    );
}

#[test]
fn splitter_handle_moves_by_keyboard() {
    let (mut h, host) = host();
    let ratio = h.ui.rt_mut().signal(0.5f32);
    let sp = splitter(
        &mut h.ui,
        host,
        "sp",
        NodeStyle::default().size(400.0, 100.0),
        Axis::Horizontal,
        ratio,
        "Split",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(sp.handle), Some(Role::Splitter));
    h.focus(sp.handle);
    h.press(KeyCode::Right, NONE);
    assert!(
        (ratio.get(h.ui.rt()) - 0.52).abs() < 1e-4,
        "{}",
        ratio.get(h.ui.rt())
    );
    h.press(KeyCode::Home, NONE);
    let w_first = h.ui.rect(sp.first).unwrap_or_default().w;
    h.press(KeyCode::End, NONE);
    assert!(
        h.ui.rect(sp.first).unwrap_or_default().w > w_first,
        "the panes resized"
    );
}

#[test]
fn tabs_switch_reorder_and_ask_to_close() {
    let (mut h, host) = host();
    let sel = h.ui.rt_mut().signal(0usize);
    let tabs = add(
        &mut h,
        host,
        "tabs",
        NodeStyle::leaf(),
        Tabs::new(&["a.rs", "b.rs", "c.rs"], sel)
            .closable()
            .reorderable(),
    );
    let pages: Vec<WidgetId> = (0..3u32)
        .map(|i| {
            h.ui.add(
                host,
                Key::Index(i),
                NodeStyle::leaf(),
                Label::new(format!("page {i}")),
            )
            .unwrap_or_else(|e| panic!("{e}"))
        })
        .collect();
    if let Some(t) = h.ui.widget_mut::<Tabs>(tabs) {
        t.set_pages(pages.clone());
    }
    for p in &pages[1..] {
        h.ui.set_hidden(*p, true).unwrap_or_else(|e| panic!("{e}"));
    }
    h.settle();
    assert_eq!(h.ui.role(tabs), Some(Role::TabList));
    assert_eq!(h.child_node(tabs, 0).map(|n| n.role()), Some(Role::Tab));
    h.focus(tabs);
    h.press(KeyCode::Right, NONE);
    assert_eq!(sel.get(h.ui.rt()), 1);
    assert!(h.ui.is_hidden(pages[0]) && !h.ui.is_hidden(pages[1]));
    let ctrl_shift = Modifiers {
        ctrl: true,
        shift: true,
        ..NONE
    };
    h.press(KeyCode::Right, ctrl_shift);
    let t = h.ui.widget::<Tabs>(tabs).unwrap_or_else(|| panic!("tabs"));
    assert_eq!(
        t.titles(),
        ["a.rs", "c.rs", "b.rs"],
        "Ctrl+Shift+Right moved the tab"
    );
    assert_eq!(t.pages()[2], pages[1], "its page moved with it");
    assert_eq!(sel.get(h.ui.rt()), 2, "the moved tab stays selected");
    assert_eq!(h.take::<TabMoved>().len(), 1);
    h.press(KeyCode::Char('w'), Modifiers::CTRL);
    let closed = h.take::<TabClosed>();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].title, "b.rs");
    assert_eq!(
        h.ui.widget::<Tabs>(tabs).map(|t| t.titles().len()),
        Some(3),
        "closing is the owner's decision"
    );
}

#[test]
fn collapsible_section_hides_its_body() {
    let (mut h, host) = host();
    let open = h.ui.rt_mut().signal(true);
    let body = host.child(&Key::Static("body"));
    let mut hd = CollapsibleHeader::new("Physics", open);
    hd.set_body(body);
    let head = add(&mut h, host, "head", NodeStyle::leaf(), hd);
    add(&mut h, host, "body", NodeStyle::leaf(), Label::new("Mass"));
    assert_eq!(h.ui.role(head), Some(Role::Button));
    h.focus(head);
    h.press(KeyCode::Enter, NONE);
    assert!(!open.get(h.ui.rt()) && h.ui.is_hidden(body));
    assert_eq!(h.node(head).and_then(|n| n.is_expanded()), Some(false));
    h.press(KeyCode::Right, NONE);
    assert!(open.get(h.ui.rt()) && !h.ui.is_hidden(body));
}

#[test]
fn card_group_box_and_grid_lay_out_as_containers() {
    let (mut h, host) = host();
    let space = h.ui.theme().space;
    let card = add(
        &mut h,
        host,
        "card",
        NodeStyle::card(space[1], space[3]),
        Container::new(Role::Group).labelled("Card"),
    );
    let gstyle = GroupBox::style(h.ui.theme());
    let gb = add(&mut h, host, "gb", gstyle, GroupBox::new("Group"));
    let grid = add(
        &mut h,
        host,
        "grid",
        NodeStyle::grid(3, 4.0).width(300.0),
        Container::new(Role::Group).labelled("Grid"),
    );
    let cells: Vec<WidgetId> = (0..6u32)
        .map(|i| {
            h.ui.add(
                grid,
                Key::Index(i),
                NodeStyle::leaf(),
                Badge::new(format!("{i}")),
            )
            .unwrap_or_else(|e| panic!("{e}"))
        })
        .collect();
    h.settle();
    assert_eq!(label_of(&mut h, card), "Card");
    assert_eq!(h.ui.role(gb), Some(Role::Group));
    assert_eq!(label_of(&mut h, gb), "Group");
    let r: Vec<_> = cells
        .iter()
        .map(|c| h.ui.rect(*c).unwrap_or_default())
        .collect();
    assert_eq!(r[0].y, r[2].y, "three cells per row");
    assert!(r[3].y > r[0].y, "the fourth wraps to the next row");
    assert_eq!(r[0].x, r[3].x);
}

// ---- menus & overlays --------------------------------------------------------------------

#[test]
fn menu_bar_opens_moves_and_chooses_by_keyboard() {
    let (mut h, host) = host();
    let bar = add(
        &mut h,
        host,
        "bar",
        NodeStyle::leaf(),
        MenuBar::new(vec![
            (
                "File",
                vec![
                    MenuItem::action("new", "New"),
                    MenuItem::action("open", "Open"),
                ],
            ),
            (
                "Edit",
                vec![
                    MenuItem::action("undo", "Undo"),
                    MenuItem::action("redo", "Redo").disabled(),
                    MenuItem::action("cut", "Cut"),
                ],
            ),
        ]),
    );
    assert_eq!(h.ui.role(bar), Some(Role::MenuBar));
    h.focus(bar);
    h.press(KeyCode::Down, NONE);
    assert_eq!(
        h.ui.widget::<MenuBar>(bar).and_then(|b| b.open_menu()),
        Some(0)
    );
    let menu = h.ui.popups()[0];
    assert_eq!(h.ui.role(menu), Some(Role::Menu));
    h.press(KeyCode::Right, NONE);
    assert_eq!(
        h.ui.widget::<MenuBar>(bar).and_then(|b| b.open_menu()),
        Some(1),
        "Right switches to the next menu"
    );
    h.press(KeyCode::Home, NONE);
    h.press(KeyCode::Down, NONE); // skips the disabled Redo
    h.take::<MenuChosen>();
    h.press(KeyCode::Enter, NONE);
    let chosen = h.take::<MenuChosen>();
    assert_eq!(
        chosen.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec!["cut"]
    );
    assert!(h.ui.popups().is_empty(), "choosing closes the menu chain");
    assert_eq!(h.ui.focused(), Some(bar), "focus returns");
    h.press(KeyCode::Enter, NONE);
    h.press(KeyCode::Escape, NONE);
    assert!(h.ui.popups().is_empty(), "Escape closes");
}

#[test]
fn submenu_opens_right_closes_left_and_chooses_back_to_the_bar() {
    let (mut h, host) = host();
    h.ui.a11y_activate();
    let bar = add(
        &mut h,
        host,
        "bar",
        NodeStyle::leaf(),
        MenuBar::new(vec![
            (
                "File",
                vec![
                    MenuItem::action("new", "New"),
                    MenuItem::submenu(
                        "Recent",
                        vec![
                            MenuItem::action("r1", "island.forge"),
                            MenuItem::action("r2", "moon.forge"),
                        ],
                    ),
                    MenuItem::action("quit", "Quit"),
                ],
            ),
            ("Edit", vec![MenuItem::action("undo", "Undo")]),
        ]),
    );
    h.focus(bar);
    h.press(KeyCode::Down, NONE);
    let file = h.ui.popups()[0];
    h.press(KeyCode::Home, NONE);
    h.press(KeyCode::Down, NONE); // onto "Recent"
    let recent = h
        .child_node(file, 1)
        .unwrap_or_else(|| panic!("Recent has an a11y node"));
    assert_eq!(recent.role(), Role::MenuItem);
    assert_eq!(recent.label(), Some("Recent"));
    assert_eq!(recent.has_popup(), Some(accesskit::HasPopup::Menu));
    assert_eq!(recent.is_expanded(), Some(false), "collapsed until opened");

    // Right opens the submenu as a second Menu popup; the File menu stays open.
    h.press(KeyCode::Right, NONE);
    assert_eq!(h.ui.popups().len(), 2, "Right opens the submenu");
    let sub = h.ui.popups()[1];
    assert_eq!(h.ui.role(sub), Some(Role::Menu));
    assert_eq!(
        h.child_node(file, 1).and_then(|n| n.is_expanded()),
        Some(true),
        "the parent item reports its submenu open"
    );
    assert_eq!(
        h.ui.widget::<MenuBar>(bar).and_then(|b| b.open_menu()),
        Some(0),
        "Right on a submenu item opens it instead of switching bar menus"
    );
    assert_eq!(
        h.child_node(sub, 0)
            .and_then(|n| n.label().map(str::to_string)),
        Some("island.forge".to_string())
    );

    // Left closes only the submenu and leaves File open on "Recent".
    h.press(KeyCode::Left, NONE);
    assert_eq!(h.ui.popups(), vec![file], "Left closes just the submenu");
    assert_eq!(
        h.ui.widget::<MenuBar>(bar).and_then(|b| b.open_menu()),
        Some(0),
        "Left in a submenu does not switch bar menus"
    );
    assert_eq!(
        h.child_node(file, 1).and_then(|n| n.is_expanded()),
        Some(false)
    );

    // Reopen, move to the second entry, choose: one MenuChosen, the whole chain closes and
    // focus returns to the bar.
    h.press(KeyCode::Right, NONE);
    h.press(KeyCode::Home, NONE);
    h.press(KeyCode::Down, NONE);
    h.take::<MenuChosen>();
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        h.take::<MenuChosen>()
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["r2"]
    );
    assert!(h.ui.popups().is_empty(), "choosing closes the menu chain");
    assert_eq!(h.ui.focused(), Some(bar), "focus returns to the bar");
}

#[test]
fn context_menu_opens_from_the_keyboard() {
    let (mut h, host) = host();
    let area = add(
        &mut h,
        host,
        "a",
        NodeStyle::leaf().size(160.0, 60.0),
        ContextMenuArea::new(
            "Canvas",
            vec![
                MenuItem::action("copy", "Copy"),
                MenuItem::action("paste", "Paste"),
            ],
        ),
    );
    assert_eq!(h.ui.role(area), Some(Role::Group));
    let an = h
        .node(area)
        .unwrap_or_else(|| panic!("area has no a11y node"));
    assert_eq!(an.role(), Role::Group);
    assert_eq!(an.label(), Some("Canvas"));
    assert_eq!(
        an.has_popup(),
        Some(accesskit::HasPopup::Menu),
        "the area announces that it opens a menu"
    );
    h.focus(area);
    h.press(KeyCode::F10, Modifiers::SHIFT);
    assert_eq!(h.ui.popups().len(), 1, "Shift+F10 opens it");
    let menu = h.ui.popups()[0];
    assert_eq!(h.ui.role(menu), Some(Role::Menu));
    assert_eq!(h.node(menu).map(|n| n.role()), Some(Role::Menu));
    h.press(KeyCode::End, NONE);
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        h.take::<MenuChosen>()
            .into_iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        vec!["paste".to_string()]
    );
}

#[test]
fn popover_opens_and_escape_returns_focus() {
    let (mut h, host) = host();
    let b = add(
        &mut h,
        host,
        "p",
        NodeStyle::leaf(),
        PopoverButton::new("Info", "Details here.").size(Size::new(200.0, 80.0)),
    );
    h.focus(b);
    h.press(KeyCode::Enter, NONE);
    assert!(h.ui.widget::<PopoverButton>(b).is_some_and(|p| p.is_open()));
    let pop = h.ui.popups()[0];
    assert_eq!(h.ui.role(pop), Some(Role::Dialog));
    h.press(KeyCode::Escape, NONE);
    assert!(h.ui.popups().is_empty());
    assert_eq!(h.ui.focused(), Some(b));
}

#[test]
fn destructive_dialog_names_the_loss_traps_focus_and_defaults_to_cancel() {
    let (mut h, host) = host();
    let owner = add(&mut h, host, "o", NodeStyle::leaf(), Button::new("Delete"));
    assert!(
        DialogSpec::destructive("x", "Delete?", "  ", "Delete").is_err(),
        "no loss named: refused"
    );
    let spec = DialogSpec::destructive(
        "del",
        "Delete layer?",
        "12 painted tiles will be lost.",
        "Delete",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let dlg = open_dialog(&mut h.ui, owner, spec).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.role(dlg), Some(Role::AlertDialog));
    let desc = h
        .node(dlg)
        .and_then(|n| n.description().map(str::to_string))
        .unwrap_or_default();
    assert!(
        desc.contains("12 painted tiles"),
        "the loss is announced: {desc:?}"
    );
    let first = h.ui.focused();
    assert_eq!(
        first
            .and_then(|f| h.node(f))
            .and_then(|n| n.label().map(str::to_string))
            .as_deref(),
        Some("Cancel"),
        "the safe default"
    );
    for _ in 0..6 {
        h.tab(false);
        h.settle();
        let f = h.ui.focused().unwrap_or(owner);
        let mut cur = Some(f);
        let mut inside = false;
        while let Some(c) = cur {
            inside |= c == dlg;
            cur = h.ui.parent(c);
        }
        assert!(inside, "Tab left the modal");
    }
    h.take::<DialogResult>();
    h.press(KeyCode::Escape, NONE);
    let r = h.take::<DialogResult>();
    assert_eq!(
        r,
        vec![DialogResult {
            dialog: "del".into(),
            confirmed: false
        }]
    );
    assert!(h.ui.popups().is_empty());
}

#[test]
fn toasts_announce_and_expire_on_one_timer() {
    let (mut h, _host) = host();
    let t =
        h.ui.toast("Saved", Severity::Success)
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.toasts(), vec![t]);
    assert!(
        h.node(t).is_some_and(|n| n.live().is_some()),
        "a live region"
    );
    let wakes = h.wakeups;
    h.advance(TOAST_DURATION + Duration::from_millis(10));
    assert!(h.ui.toasts().is_empty(), "expired");
    assert!(
        h.wakeups - wakes <= 2,
        "expiry is a deadline, not polling ({} wakeups)",
        h.wakeups - wakes
    );
}

#[test]
fn tooltip_appears_after_the_delay_only() {
    let (mut h, host) = host();
    let b = add(
        &mut h,
        host,
        "b",
        NodeStyle::leaf(),
        IconButton::new("ⓘ", "More information"),
    );
    let r = h.ui.rect(b).unwrap_or_default();
    h.move_to(r.center());
    h.advance(forge_ui::overlay::TOOLTIP_DELAY / 2);
    assert!(h.ui.tooltip_shown().is_none());
    assert!(
        h.node(b).is_some_and(|n| n.described_by().is_empty()),
        "no tooltip yet: nothing describes the button"
    );
    h.advance(forge_ui::overlay::TOOLTIP_DELAY);
    let Some((owner, bubble)) = h.ui.tooltip_shown() else {
        panic!("tooltip did not appear after the delay");
    };
    assert_eq!(owner, b);
    assert_eq!(h.ui.role(bubble), Some(Role::Tooltip));
    let bn = h
        .node(bubble)
        .unwrap_or_else(|| panic!("bubble has no a11y node"));
    assert_eq!(bn.role(), Role::Tooltip);
    assert_eq!(bn.label(), Some("More information"));
    let on = h
        .node(b)
        .unwrap_or_else(|| panic!("button has no a11y node"));
    assert_eq!(
        on.described_by(),
        &[bubble.to_accesskit()],
        "the owner is described by the bubble"
    );
    assert_eq!(on.description(), Some("More information"));
    // Leaving hides it and the owner's description relation goes with it.
    h.move_to(forge_ui::Point::new(-50.0, -50.0));
    h.settle();
    assert!(h.ui.tooltip_shown().is_none());
    assert!(h.node(b).is_some_and(|n| n.described_by().is_empty()));
}

// ---- feedback ----------------------------------------------------------------------------

#[test]
fn progress_bar_reports_value_and_animates_only_when_busy_and_visible() {
    let (mut h, host) = host();
    let p = h.ui.rt_mut().signal(Progress::Fraction(0.6));
    let id = add(
        &mut h,
        host,
        "p",
        NodeStyle::leaf().width(200.0),
        ProgressBar::new(p, "Import"),
    );
    assert_eq!(h.ui.role(id), Some(Role::ProgressIndicator));
    assert!((h.node(id).and_then(|n| n.numeric_value()).unwrap_or(0.0) - 60.0).abs() < 1e-3);
    assert!(
        !h.ui
            .widget::<ProgressBar>(id)
            .is_some_and(|b| b.is_animating())
    );
    p.set(h.ui.rt_mut(), Progress::Indeterminate);
    h.settle();
    assert!(
        h.ui.widget::<ProgressBar>(id)
            .is_some_and(|b| b.is_animating())
    );
    assert!(h.node(id).is_some_and(|n| n.is_busy()));
    h.ui.set_hidden(id, true).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.advance(Duration::from_secs(2)), 0, "hidden: no frames");
    h.ui.set_hidden(id, false).unwrap_or_else(|e| panic!("{e}"));
    h.ui.set_reduced_motion(true);
    h.settle();
    let frames = h.advance(Duration::from_secs(1));
    assert!(
        frames <= 1,
        "reduced motion: a static busy bar ({frames} frames)"
    );
}

#[test]
fn spinner_skeleton_and_empty_state() {
    let (mut h, host) = host();
    let live = h.ui.rt_mut().signal(true);
    let s = add(
        &mut h,
        host,
        "s",
        NodeStyle::leaf(),
        Spinner::new(live, "Loading"),
    );
    let k = add(
        &mut h,
        host,
        "k",
        NodeStyle::leaf().width(160.0),
        Skeleton::new(3, "Loading details"),
    );
    let e = add(
        &mut h,
        host,
        "e",
        NodeStyle::leaf().width(240.0),
        EmptyState::new("No assets yet").action("Import…"),
    );
    assert_eq!(h.ui.role(s), Some(Role::ProgressIndicator));
    assert_eq!(label_of(&mut h, s), "Loading");
    assert_eq!(h.ui.role(k), Some(Role::ProgressIndicator));
    assert_eq!(
        h.ui.role(e),
        Some(Role::Button),
        "an empty state with an action is operable"
    );
    h.focus(e);
    h.press(KeyCode::Enter, NONE);
    assert_eq!(h.take::<EmptyStateAction>().len(), 1);
}

// ---- navigation --------------------------------------------------------------------------

#[test]
fn breadcrumb_moves_and_chooses_a_segment() {
    let (mut h, host) = host();
    let segs = h.ui.rt_mut().signal(vec![
        "Project".to_string(),
        "Levels".into(),
        "Forest".into(),
    ]);
    let id = add(
        &mut h,
        host,
        "b",
        NodeStyle::leaf(),
        Breadcrumb::new(segs, "Location"),
    );
    assert_eq!(h.ui.role(id), Some(Role::Navigation));
    let last = h
        .child_node(id, 2)
        .unwrap_or_else(|| panic!("segment node"));
    assert_eq!(last.role(), Role::Link);
    assert_eq!(last.aria_current(), Some(accesskit::AriaCurrent::Location));
    h.focus(id);
    h.press(KeyCode::Left, NONE);
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        h.take::<BreadcrumbChosen>(),
        vec![BreadcrumbChosen {
            breadcrumb: id,
            index: 1
        }]
    );
}

#[test]
fn status_bar_is_a_polite_live_region() {
    let (mut h, host) = host();
    let a = h.ui.rt_mut().signal(String::from("Ready"));
    let id = add(
        &mut h,
        host,
        "s",
        NodeStyle::leaf().width(400.0),
        StatusBar::new(vec![StatusItem {
            text: a,
            right: false,
        }]),
    );
    assert_eq!(h.ui.role(id), Some(Role::Status));
    assert!(!h.ui.is_focusable(id));
    assert_eq!(
        h.node(id).and_then(|n| n.live()),
        Some(accesskit::Live::Polite)
    );
    a.set(h.ui.rt_mut(), String::from("Saved"));
    assert_eq!(
        h.node(id)
            .and_then(|n| n.value().map(str::to_string))
            .as_deref(),
        Some("Saved")
    );
}

#[test]
fn toolbar_is_one_tab_stop_with_arrow_roving() {
    let (mut h, host) = host();
    let before = add(
        &mut h,
        host,
        "before",
        NodeStyle::leaf(),
        Button::new("Before"),
    );
    let tb = add(
        &mut h,
        host,
        "tb",
        NodeStyle::row(4.0),
        Toolbar::new("Tools"),
    );
    let btns: Vec<WidgetId> = ["Move", "Rotate", "Scale"]
        .iter()
        .enumerate()
        .map(|(i, l)| {
            h.ui.add(
                tb,
                Key::Index(i as u32),
                NodeStyle::leaf(),
                IconButton::new("•", l),
            )
            .unwrap_or_else(|e| panic!("{e}"))
        })
        .collect();
    let after = add(
        &mut h,
        host,
        "after",
        NodeStyle::leaf(),
        Button::new("After"),
    );
    assert_eq!(h.ui.role(tb), Some(Role::Toolbar));
    h.focus(before);
    h.tab(false);
    h.settle();
    assert_eq!(h.ui.focused(), Some(btns[0]));
    h.press(KeyCode::Right, NONE);
    h.press(KeyCode::Right, NONE);
    assert_eq!(h.ui.focused(), Some(btns[2]));
    h.press(KeyCode::Right, NONE);
    assert_eq!(h.ui.focused(), Some(btns[0]), "roving wraps");
    h.tab(false);
    h.settle();
    assert_eq!(h.ui.focused(), Some(after), "one Tab leaves the toolbar");
}

#[test]
fn command_palette_filters_fuzzily_and_invokes() {
    let (mut h, host) = host();
    let b = add(
        &mut h,
        host,
        "p",
        NodeStyle::leaf(),
        CommandPaletteButton::new(vec![
            PickItem::new("file.open", "Open project").detail("Ctrl+O"),
            PickItem::new("view.profiler", "Open profiler").detail("Ctrl+Alt+P"),
            PickItem::new("edit.undo", "Undo").detail("Ctrl+Z"),
        ]),
    );
    assert_eq!(label_of(&mut h, b), "Command palette");
    h.focus(b);
    h.press(KeyCode::Enter, NONE);
    let pop = h.ui.popups()[0];
    assert_eq!(h.ui.role(pop), Some(Role::ListBox));
    h.type_text("oprof");
    h.settle();
    let shown =
        h.ui.widget::<PickList>(pop)
            .map(|p| p.shown())
            .unwrap_or_default();
    assert_eq!(
        shown.first(),
        Some(&1),
        "\"oprof\" ranks Open profiler first ({shown:?})"
    );
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        h.take::<CommandInvoked>(),
        vec![CommandInvoked {
            id: "view.profiler".into()
        }]
    );
    assert!(h.ui.popups().is_empty());
    // Opened from application code too (a global shortcut).
    let root = h.ui.root();
    let p = open_command_palette_ui(&mut h.ui, root, Rc::new(vec![PickItem::new("a", "Alpha")]));
    assert!(p.is_some());
}
