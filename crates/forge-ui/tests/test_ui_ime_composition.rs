//! `test_ui_ime_composition` (Ch.21 §21.9, §21.23; DoD M2-21).
//!
//! Drives a synthetic Japanese composition — pre-edit, candidate change, commit, then a
//! cancelled composition — into the gallery's text field and asserts that the model sees
//! exactly the committed text, once. Also: IME is enabled only while a text widget has
//! focus, and the caret rect is reported so the candidate window follows the caret.
//!
//! Positive control: a field that writes pre-edit text into the model fails.

use std::cell::RefCell;
use std::rc::Rc;

use forge_ui::gallery::{self, Gallery, GalleryFaults};
use forge_ui::testing::Harness;
use forge_ui::widgets::TextField;
use forge_ui::{ImeEvent, UiConfig};

fn harness(faults: GalleryFaults) -> (Harness, Gallery, Rc<RefCell<Vec<String>>>) {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, faults).unwrap_or_else(|e| panic!("{e}"));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s2 = seen.clone();
    let name = g.name;
    // Every value the model takes, as the rest of the app would observe it.
    h.ui.rt_mut()
        .effect(move |rt| s2.borrow_mut().push(name.get(rt)));
    h.settle();
    seen.borrow_mut().clear();
    (h, g, seen)
}

fn preedit(h: &mut Harness, s: &str, cursor: Option<(usize, usize)>) {
    h.ime(ImeEvent::Preedit {
        text: s.into(),
        cursor,
    });
    h.step();
}

/// The composition; returns the model values observed.
fn compose(h: &mut Harness, g: &Gallery, seen: &Rc<RefCell<Vec<String>>>) -> Result<(), String> {
    h.ui.set_focus(Some(g.text_field), false);
    h.step();
    let req =
        h.ui.take_ime_request()
            .ok_or("no IME request after focusing a text field")?;
    if !req.allowed {
        return Err("IME not enabled for a focused text field".into());
    }
    h.ime(ImeEvent::Enabled);
    for (s, c) in [("k", 1), ("か", 3), ("かn", 4), ("かん", 6), ("かんじ", 9)] {
        preedit(h, s, Some((c, c)));
        let field = h.ui.widget::<TextField>(g.text_field).ok_or("no field")?;
        if field.preedit() != Some(s) {
            return Err(format!(
                "pre-edit shown is {:?}, expected {s:?}",
                field.preedit()
            ));
        }
    }
    // Candidate change: the IME converts to kanji, still uncommitted.
    preedit(h, "漢字", Some((0, 6)));
    if !seen.borrow().is_empty() {
        return Err(format!(
            "the model saw pre-edit text before commit: {:?}",
            seen.borrow()
        ));
    }
    // The caret area was reported while composing.
    let req = h.ui.take_ime_request();
    if req.is_none_or(|r| r.caret.is_none()) {
        return Err("no caret rect reported during composition".into());
    }
    h.ime(ImeEvent::Commit("漢字".into()));
    h.step();
    // A second composition, cancelled (pre-edit cleared without a commit).
    preedit(h, "に", Some((3, 3)));
    preedit(h, "", None);
    h.settle();
    let field = h.ui.widget::<TextField>(g.text_field).ok_or("no field")?;
    if field.preedit().is_some() {
        return Err("a cancelled composition left pre-edit text".into());
    }
    let values = seen.borrow().clone();
    if values != vec!["漢字".to_string()] {
        return Err(format!(
            "the model saw {values:?}; expected exactly [\"漢字\"] once"
        ));
    }
    Ok(())
}

#[test]
fn composition_commits_exactly_once() {
    let (mut h, g, seen) = harness(GalleryFaults::default());
    compose(&mut h, &g, &seen).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(g.name.get(h.ui.rt()), "漢字");
    // The caret sits after the committed text.
    let f = h.ui.widget::<TextField>(g.text_field).map(|f| f.cursor());
    assert_eq!(f, Some("漢字".len()));
}

#[test]
fn ime_is_enabled_only_while_a_text_widget_has_focus() {
    let (mut h, g, _seen) = harness(GalleryFaults::default());
    h.ui.set_focus(Some(g.text_field), true);
    h.step();
    assert!(h.ui.take_ime_request().is_some_and(|r| r.allowed));
    h.ui.set_focus(Some(g.save), true);
    h.step();
    assert!(h.ui.take_ime_request().is_some_and(|r| !r.allowed));
    // Nothing changed since: no request is re-sent.
    h.step();
    assert!(h.ui.take_ime_request().is_none());
}

#[test]
fn positive_control_preedit_committed_early_fails() {
    let (mut h, g, seen) = harness(GalleryFaults {
        field_commits_preedit_early: true,
        ..GalleryFaults::default()
    });
    let e = compose(&mut h, &g, &seen).expect_err("a field committing pre-edit text must fail");
    assert!(e.contains("pre-edit text before commit"), "{e}");
}
