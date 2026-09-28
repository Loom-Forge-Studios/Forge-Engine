//! `ui_gallery` shows the whole §21.16 catalogue (DoD M2-24), and every page of it meets
//! the catalogue rules as a user meets them:
//!
//! * every catalogue widget the plan lists is in the gallery (completeness);
//! * on every page, every focusable widget has a role, an accessible name and the Focus
//!   action, and is reached by Tab from the page's first stop (keyboard operation);
//! * every page idles at zero frames and zero wakeups once settled (D-5), except while
//!   the Feedback page's busy indicators are on screen — and they stop the moment it is
//!   hidden;
//! * the gallery's menu bar carries a submenu that opens, closes and chooses by keyboard.
//!
//! Positive controls (W2): a catalogue missing one widget fails completeness
//! (`positive_control_a_missing_widget_fails_completeness`); a container wrongly declared a
//! focus scope traps Tab and fails reachability
//! (`positive_control_a_tab_trap_fails_keyboard_reachability`); a curve editor that never
//! stops requesting frames fails the idle check
//! (`positive_control_a_widget_that_never_idles_fails_the_idle_check`).

use std::collections::BTreeSet;
use std::time::Duration;

use forge_ui::gallery::{self, Gallery, GalleryFaults};
use forge_ui::gallery_catalogue::GROUPS;
use forge_ui::testing::Harness;
use forge_ui::{Role, UiConfig, WidgetId};

/// Every catalogue widget §21.16 requires of WP-U2, by its gallery name. (The node canvas
/// is WP-U8's; the dock tab strip is WP-U3's.)
const REQUIRED: &[&str] = &[
    // Text & display
    "label",
    "rich_text",
    "icon",
    "image",
    "separator",
    "badge",
    "shortcut",
    // Buttons & toggles
    "btn_primary",
    "btn_secondary",
    "btn_ghost",
    "btn_danger",
    "btn_small",
    "btn_large",
    "icon_button",
    "toggle_button",
    "tri_checkbox",
    "checkbox",
    "radio",
    "switch",
    "segmented",
    // Numeric
    "slider",
    "slider_log",
    "numeric",
    "spin_box",
    "range_slider",
    // Text entry
    "text_field",
    "password",
    "multiline",
    "search",
    "path",
    // Choice
    "combo",
    "chips",
    "autocomplete",
    // Collections
    "list",
    "tree",
    "table",
    // Containers
    "scroll_area",
    "splitter",
    "doc_tabs",
    "collapsible",
    "card",
    "group_box",
    "grid",
    // Menus & overlays (the submenu lives inside the menu bar: see
    // `the_gallery_menu_bar_carries_a_working_submenu`)
    "menu_bar",
    "context_area",
    "tooltip_button",
    "popover",
    "dialog_button",
    "toast_button",
    // Feedback
    "progress",
    "progress_busy",
    "spinner",
    "skeleton",
    "empty_state",
    // Editors
    "color_button",
    "color_picker",
    "vector",
    "quat",
    "frame_pos",
    "curve",
    "gradient",
    "asset_ref",
    "property_grid",
    // Navigation
    "breadcrumb",
    "status_bar",
    // Graph (WP-U8)
    "node_canvas",
    "toolbar",
    "palette",
];

/// Required widgets the built gallery does not really contain: no id recorded, or an id
/// that is not a live widget with a role in the tree, or one that sits on no catalogue page.
fn missing(h: &Harness, g: &Gallery) -> Vec<&'static str> {
    REQUIRED
        .iter()
        .copied()
        .filter(|n| {
            let Some(id) = g.catalogue.ids.get(n).copied() else {
                return true;
            };
            let live = h.ui.contains(id) && h.ui.role(id).is_some();
            let on_a_page = {
                let mut cur = Some(id);
                let mut found = false;
                while let Some(c) = cur {
                    if g.catalogue.pages.contains(&c) && c != id {
                        found = true;
                        break;
                    }
                    cur = h.ui.parent(c);
                }
                found
            };
            !(live && on_a_page)
        })
        .collect()
}

fn harness() -> (Harness, Gallery) {
    harness_with(GalleryFaults::default())
}

fn harness_with(faults: GalleryFaults) -> (Harness, Gallery) {
    let mut h = Harness::new(UiConfig {
        size: forge_ui::Size::new(1280.0, 1000.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, faults).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, g)
}

fn show(h: &mut Harness, g: &Gallery, page: usize) {
    g.catalogue.tab.set(h.ui.rt_mut(), page);
    for (i, p) in g.catalogue.pages.iter().enumerate() {
        h.ui.set_hidden(*p, i != page)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    h.settle();
}

fn is_inside(h: &Harness, mut w: WidgetId, root: WidgetId) -> bool {
    loop {
        if w == root {
            return true;
        }
        match h.ui.parent(w) {
            Some(p) => w = p,
            None => return false,
        }
    }
}

fn describe(h: &Harness, id: WidgetId) -> String {
    let n = h.ui.a11y_node(id);
    format!(
        "{:?} {:?}",
        n.map(|n| n.role()),
        n.and_then(|n| n.label().map(str::to_string))
    )
}

/// Walks every page as a keyboard user: returns one line per problem (a focusable widget
/// without a role, a name or the Focus action, or one that Tab from the page's first stop
/// never reaches). Empty means every page is keyboard operable.
fn keyboard_problems(faults: GalleryFaults) -> Vec<String> {
    let (mut h, g) = harness_with(faults);
    h.ui.a11y_activate();
    let mut problems = Vec::new();
    for (page, group) in GROUPS.iter().enumerate() {
        show(&mut h, &g, page);
        let page_id = g.catalogue.pages[page];
        // Every visible focusable widget on the page: a role, a name, the Focus action.
        let focusable: BTreeSet<WidgetId> =
            h.ui.walk()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| {
                    is_inside(&h, *id, page_id) && h.ui.is_visible(*id) && h.ui.is_focusable(*id)
                })
                .collect();
        if focusable.is_empty() {
            problems.push(format!("{group}: nothing to operate"));
            continue;
        }
        for id in &focusable {
            let Some(node) = h.node(*id) else {
                problems.push(format!("{group}: {id:?} has no a11y node"));
                continue;
            };
            if node.role() == Role::Unknown {
                problems.push(format!("{group}: {id:?} has no role"));
            }
            if node.label().unwrap_or("").trim().is_empty() {
                problems.push(format!(
                    "{group}: {id:?} ({:?}) has no accessible name",
                    node.role()
                ));
            }
            if !node.supports_action(accesskit::Action::Focus) {
                problems.push(format!("{group}: {id:?} lacks Focus"));
            }
        }
        // Tab from the page's first stop reaches every one of them (within the catalogue
        // panel's focus scope, which wraps).
        let first =
            h.ui.walk()
                .into_iter()
                .map(|(id, _)| id)
                .find(|id| focusable.contains(id));
        h.ui.set_focus(first, true);
        h.settle();
        let mut reached = BTreeSet::new();
        let mut order = Vec::new();
        for _ in 0..(focusable.len() * 3 + 8) {
            if let Some(f) = h.ui.focused() {
                reached.insert(f);
                order.push(describe(&h, f));
            }
            h.tab(false);
            h.settle();
        }
        // A toolbar is one tab stop: its other buttons are reached with the arrow keys.
        let in_toolbar = |h: &Harness, id: WidgetId| {
            h.ui.parent(id).and_then(|p| h.ui.role(p)) == Some(Role::Toolbar)
        };
        let roving: Vec<WidgetId> = focusable
            .iter()
            .copied()
            .filter(|id| in_toolbar(&h, *id))
            .collect();
        if let Some(entry) = roving.iter().copied().find(|id| reached.contains(id)) {
            h.ui.set_focus(Some(entry), true);
            h.settle();
            for _ in 0..roving.len() {
                h.key(forge_ui::KeyCode::Right, forge_ui::Modifiers::NONE);
                h.settle();
                if let Some(f) = h.ui.focused() {
                    reached.insert(f);
                }
            }
        }
        let unreached: Vec<String> = focusable
            .difference(&reached)
            .map(|id| describe(&h, *id))
            .collect();
        if !unreached.is_empty() {
            order.dedup();
            problems.push(format!(
                "{group}: Tab never reached {unreached:?}; order {order:?}"
            ));
        }
    }
    problems
}

/// Settles every page and counts what it does over ten idle seconds: returns one line per
/// page that draws or wakes while idle (the Feedback page must animate while shown and
/// stop the moment it is hidden). Empty means the whole catalogue idles at zero.
fn idle_problems(faults: GalleryFaults) -> Vec<String> {
    let (mut h, g) = harness_with(faults);
    let mut problems = Vec::new();
    for (page, group) in GROUPS.iter().enumerate() {
        show(&mut h, &g, page);
        h.advance(Duration::from_millis(600)); // tooltips and first-frame timers settle
        let w0 = h.wakeups;
        let frames = h.advance(Duration::from_secs(10));
        let wakeups = h.wakeups - w0;
        if *group == "Feedback" {
            if frames == 0 {
                problems.push(format!("{group}: the busy indicators do not animate"));
            }
        } else if (frames, wakeups) != (0, 0) {
            problems.push(format!("{group}: {frames} frames, {wakeups} wakeups idle"));
        }
    }
    // Leaving the Feedback page stops its animations at once.
    show(&mut h, &g, 0);
    h.advance(Duration::from_millis(100));
    let w0 = h.wakeups;
    let frames = h.advance(Duration::from_secs(10));
    let wakeups = h.wakeups - w0;
    if (frames, wakeups) != (0, 0) {
        problems.push(format!(
            "after Feedback: {frames} frames, {wakeups} wakeups (a hidden spinner or busy bar must schedule nothing)"
        ));
    }
    problems
}

#[test]
fn the_gallery_shows_the_whole_catalogue() {
    let (h, g) = harness();
    let m = missing(&h, &g);
    assert!(
        m.is_empty(),
        "catalogue widgets missing from ui_gallery: {m:?}"
    );
    assert_eq!(g.catalogue.pages.len(), GROUPS.len());
}

/// W2: a gallery really built without one widget (`GalleryFaults::omit_widget`: the widget
/// is never added to the tree) fails completeness, naming exactly that widget.
#[test]
fn positive_control_a_missing_widget_fails_completeness() {
    let (h, g) = harness_with(GalleryFaults {
        omit_widget: Some("gradient"),
        ..GalleryFaults::default()
    });
    assert!(
        h.ui.walk()
            .iter()
            .all(|(id, _)| h.ui.key_of(*id) != Some(&forge_ui::Key::Static("gradient"))),
        "the omitted widget is not in the tree"
    );
    assert_eq!(missing(&h, &g), vec!["gradient"]);
}

#[test]
fn every_page_is_named_and_keyboard_operable() {
    let problems = keyboard_problems(GalleryFaults::default());
    assert!(problems.is_empty(), "{problems:#?}");
}

/// W2 control for the keyboard check: a container wrongly declared a focus scope traps
/// Tab on the Containers page, and the check names the widget it cut off.
#[test]
fn positive_control_a_tab_trap_fails_keyboard_reachability() {
    let problems = keyboard_problems(GalleryFaults {
        doc_tabs_box_traps_tab: true,
        ..GalleryFaults::default()
    });
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(
        problems[0].starts_with("Containers: Tab never reached") && problems[0].contains("Physics"),
        "{problems:#?}"
    );
}

#[test]
fn every_page_idles_at_zero_frames() {
    let problems = idle_problems(GalleryFaults::default());
    assert!(problems.is_empty(), "{problems:#?}");
}

/// W2 control for the idle check: a curve editor that re-requests an animation frame on
/// every event keeps the Editors page busy, and the check reports that page.
#[test]
fn positive_control_a_widget_that_never_idles_fails_the_idle_check() {
    let problems = idle_problems(GalleryFaults {
        curve_never_idles: true,
        ..GalleryFaults::default()
    });
    assert!(
        problems.iter().any(|p| p.starts_with("Editors: ")),
        "the busy curve editor went unnoticed: {problems:#?}"
    );
}

/// The gallery applies the collections' model actions the way an editor panel's command
/// handler would (I7: the view only asks).
#[test]
fn gallery_applies_rename_move_and_delete_like_a_panel_would() {
    use forge_ui::gallery_catalogue::apply_action;
    use forge_ui::widgets::VirtualTree;
    use forge_ui::{KeyCode, Modifiers};
    let (mut h, g) = harness();
    show(&mut h, &g, 5);
    let list = g.catalogue.ids["list"];
    h.ui.set_focus(Some(list), true);
    h.settle();
    h.key(KeyCode::F2, Modifiers::NONE);
    h.type_text("Hero");
    h.key(KeyCode::Enter, Modifiers::NONE);
    h.key(
        KeyCode::Down,
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    );
    for a in h.ui.take_actions() {
        apply_action(&mut h.ui, &g.catalogue, &a);
    }
    h.settle();
    let t =
        h.ui.widget::<VirtualTree>(list)
            .unwrap_or_else(|| panic!("list"));
    assert_eq!(t.label_of(0), Some("Hero"), "renamed by the owner");
    assert_eq!(t.key_at(1), Some(0), "moved down one by the owner");
    h.key(KeyCode::Delete, Modifiers::NONE);
    for a in h.ui.take_actions() {
        apply_action(&mut h.ui, &g.catalogue, &a);
    }
    assert_eq!(
        h.ui.widget::<VirtualTree>(list).map(|t| t.row_count()),
        Some(999)
    );
}

/// §21.16 lists the submenu under Menus & overlays; the gallery's File menu carries one
/// ("Recent"), and it works from the keyboard as a user meets it.
#[test]
fn the_gallery_menu_bar_carries_a_working_submenu() {
    use forge_ui::widgets::MenuChosen;
    use forge_ui::{KeyCode, Modifiers};
    let (mut h, g) = harness();
    h.ui.a11y_activate();
    show(&mut h, &g, 7);
    let bar = g.catalogue.ids["menu_bar"];
    h.ui.set_focus(Some(bar), true);
    h.settle();
    h.key(KeyCode::Down, Modifiers::NONE);
    h.settle();
    let file = h.ui.popups()[0];
    // New, Open…, Recent ▸, Quit: type-ahead lands on Recent.
    h.key(KeyCode::Char('r'), Modifiers::NONE);
    h.settle();
    let recent = h
        .child_node(file, 2)
        .unwrap_or_else(|| panic!("Recent has an a11y node"));
    assert_eq!(recent.label(), Some("Recent"));
    assert_eq!(recent.has_popup(), Some(accesskit::HasPopup::Menu));
    h.key(KeyCode::Right, Modifiers::NONE);
    h.settle();
    assert_eq!(h.ui.popups().len(), 2, "Right opens the submenu");
    assert_eq!(h.ui.role(h.ui.popups()[1]), Some(Role::Menu));
    h.key(KeyCode::Left, Modifiers::NONE);
    h.settle();
    assert_eq!(h.ui.popups(), vec![file], "Left closes just the submenu");
    h.key(KeyCode::Right, Modifiers::NONE);
    h.key(KeyCode::Enter, Modifiers::NONE);
    h.settle();
    let chosen: Vec<String> =
        h.ui.take_actions()
            .iter()
            .filter_map(|a| a.get::<MenuChosen>().map(|c| c.id.clone()))
            .collect();
    assert_eq!(chosen, vec!["r1".to_string()]);
    assert!(h.ui.popups().is_empty());
    assert_eq!(h.ui.focused(), Some(bar), "focus returns to the bar");
}
