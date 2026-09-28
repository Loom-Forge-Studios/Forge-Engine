//! `test_ui_focus_traversal` (Ch.21 §21.9, §21.23; DoD M2-21).
//!
//! Walks every gallery widget by keyboard alone: Tab / Shift+Tab inside each panel (a
//! focus scope) in visual order with wrap-around, F6 between panels, arrow keys inside
//! composite widgets, a modal that traps focus, and spatial navigation.
//!
//! Positive control: a gallery in which one interactive widget is not focusable — Tab
//! skips it and the walk fails.

use std::collections::BTreeSet;

use forge_ui::gallery::{self, Gallery, GalleryFaults};
use forge_ui::testing::Harness;
use forge_ui::widgets::{Button, Container};
use forge_ui::{FocusScope, KeyCode, Modifiers, NodeStyle, UiConfig, WidgetId};

fn harness(faults: GalleryFaults) -> (Harness, Gallery) {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, faults).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, g)
}

/// Panels that contain at least one of the gallery's interactive widgets.
fn focus_panels(h: &Harness, g: &Gallery) -> usize {
    g.panels
        .iter()
        .filter(|p| {
            g.interactive.iter().any(|w| {
                let mut cur = Some(*w);
                while let Some(c) = cur {
                    if c == **p {
                        return true;
                    }
                    cur = h.ui.parent(c);
                }
                false
            })
        })
        .count()
}

/// Keyboard-only walk: F6 into each panel, Tab around it until it wraps. Returns every
/// widget focused, or the first rule it broke.
fn keyboard_walk(h: &mut Harness, g: &Gallery) -> Result<BTreeSet<WidgetId>, String> {
    let mut reached = BTreeSet::new();
    let panels = focus_panels(h, g);
    if panels < 3 {
        return Err(format!("only {panels} panels with focusable widgets"));
    }
    for _ in 0..panels {
        h.key(KeyCode::F6, Modifiers::NONE);
        let first = h.ui.focused().ok_or("F6 moved focus nowhere")?;
        if !h.ui.focused().is_some_and(|f| h.ui.is_focusable(f)) {
            return Err("F6 focused a non-focusable widget".into());
        }
        let mut in_panel = vec![first];
        for _ in 0..64 {
            h.tab(false);
            let f = h.ui.focused().ok_or("Tab lost focus")?;
            if f == first {
                break;
            }
            in_panel.push(f);
        }
        if h.ui.focused() != Some(first) {
            return Err("Tab never wrapped inside its panel".into());
        }
        // Shift+Tab walks the same cycle backwards.
        let mut back = vec![first];
        for _ in 1..in_panel.len() {
            h.tab(true);
            back.push(h.ui.focused().ok_or("Shift+Tab lost focus")?);
        }
        let mut fwd_rev: Vec<_> = in_panel[1..].to_vec();
        fwd_rev.reverse();
        if back[1..] != fwd_rev[..] {
            return Err(format!(
                "Shift+Tab order {back:?} is not the reverse of Tab {in_panel:?}"
            ));
        }
        // Visual order: each Tab stop is not above-left of the previous one's row.
        for w in in_panel.windows(2) {
            let (a, b) = (
                h.ui.rect(w[0]).unwrap_or_default(),
                h.ui.rect(w[1]).unwrap_or_default(),
            );
            let same_row = (a.y - b.y).abs() < a.h.min(b.h) * 0.5;
            if !(b.y > a.y + 0.5 * a.h.min(b.h) || (same_row && b.x > a.x)) {
                return Err(format!("Tab went against visual order: {a:?} -> {b:?}"));
            }
        }
        reached.extend(in_panel);
    }
    Ok(reached)
}

fn check_all_reached(g: &Gallery, reached: &BTreeSet<WidgetId>) -> Result<(), String> {
    let missing: Vec<_> = g
        .interactive
        .iter()
        .filter(|w| !reached.contains(w))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("keyboard walk never reached {missing:?}"))
    }
}

#[test]
fn every_gallery_widget_is_reachable_by_keyboard_alone() {
    let (mut h, g) = harness(GalleryFaults::default());
    let reached = keyboard_walk(&mut h, &g).unwrap_or_else(|e| panic!("{e}"));
    check_all_reached(&g, &reached).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(reached.len(), g.interactive.len());
    // Keyboard focus draws the ring.
    let f = h.ui.focused().unwrap_or(WidgetId::ROOT);
    h.step();
    assert!(h.ui.display_list().len() > 10);
    assert!(h.ui.is_focusable(f));
}

#[test]
fn positive_control_a_skipped_widget_fails_the_walk() {
    let (mut h, g) = harness(GalleryFaults {
        checkbox_unfocusable: true,
        ..GalleryFaults::default()
    });
    let reached = keyboard_walk(&mut h, &g).unwrap_or_else(|e| panic!("{e}"));
    let e = check_all_reached(&g, &reached).expect_err("a widget Tab skips must fail the walk");
    assert!(e.contains(&format!("{:?}", g.checkbox)), "{e}");
}

#[test]
fn f6_cycles_panels_and_restores_each_panels_focus() {
    let (mut h, g) = harness(GalleryFaults::default());
    h.key(KeyCode::F6, Modifiers::NONE); // buttons panel
    h.tab(false);
    let second = h.ui.focused();
    for _ in 0..focus_panels(&h, &g) {
        h.key(KeyCode::F6, Modifiers::NONE);
    }
    assert_eq!(
        h.ui.focused(),
        second,
        "returning to a panel restores its last focus"
    );
}

#[test]
fn arrows_move_inside_composites() {
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.set_focus(Some(g.radio), true);
    let before = g.space.get(h.ui.rt());
    h.key(KeyCode::Down, Modifiers::NONE);
    h.settle();
    assert_eq!(g.space.get(h.ui.rt()), (before + 1) % 3);
    assert_eq!(
        h.ui.focused(),
        Some(g.radio),
        "arrows stay inside the composite"
    );
    // Tabs: Right selects the next tab and shows its page.
    h.ui.set_focus(Some(g.tabs), true);
    h.key(KeyCode::Right, Modifiers::NONE);
    h.settle();
    assert_eq!(g.tab.get(h.ui.rt()), 1);
    assert!(h.ui.is_visible(g.pages[1]) && !h.ui.is_visible(g.pages[0]));
    // Slider: arrows step.
    h.ui.set_focus(Some(g.slider), true);
    let v = g.opacity.get(h.ui.rt());
    h.key(KeyCode::Left, Modifiers::NONE);
    assert!((g.opacity.get(h.ui.rt()) - (v - 0.05)).abs() < 1e-4);
}

#[test]
fn a_modal_traps_focus() {
    let (mut h, g) = harness(GalleryFaults::default());
    let modal =
        h.ui.add(
            h.ui.root(),
            "modal",
            NodeStyle::row(8.0).scope(FocusScope::Modal),
            Container::new(forge_ui::Role::Dialog).labelled("Confirm"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let ok =
        h.ui.add(modal, "ok", NodeStyle::leaf(), Button::new("OK"))
            .unwrap_or_else(|e| panic!("{e}"));
    let cancel =
        h.ui.add(modal, "cancel", NodeStyle::leaf(), Button::new("Cancel"))
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    h.ui.set_focus(Some(g.save), true);
    let mut seen = BTreeSet::new();
    for _ in 0..6 {
        h.tab(false);
        seen.insert(h.ui.focused());
    }
    assert_eq!(
        seen,
        [Some(ok), Some(cancel)]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    h.key(KeyCode::F6, Modifiers::NONE);
    assert!(
        h.ui.focused() == Some(ok) || h.ui.focused() == Some(cancel),
        "F6 escaped the modal"
    );
}

#[test]
fn spatial_navigation_moves_in_the_pressed_direction() {
    let (mut h, g) = harness(GalleryFaults::default());
    h.ui.spatial_arrows = true;
    h.ui.set_focus(Some(g.save), true);
    h.key(KeyCode::Right, Modifiers::NONE);
    assert_eq!(h.ui.focused(), Some(g.cancel));
    h.key(KeyCode::Left, Modifiers::NONE);
    assert_eq!(h.ui.focused(), Some(g.save));
}
