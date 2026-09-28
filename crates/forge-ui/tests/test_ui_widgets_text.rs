//! Catalogue widget tests, part 2 (§21.16; DoD M2-24): text entry and choice.

use std::time::Duration;

use forge_ui::testing::Harness;
use forge_ui::widgets::*;
use forge_ui::{ImeEvent, InputEvent, KeyCode, Modifiers, NodeStyle, Role, Signal, WidgetId};

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
fn value_of(h: &mut Harness, id: WidgetId) -> String {
    h.node(id)
        .and_then(|n| n.value().map(str::to_string))
        .unwrap_or_default()
}

// ---- text entry --------------------------------------------------------------------------

#[test]
fn text_field_types_undoes_and_composes() {
    let (mut h, host) = host();
    let s = h.ui.rt_mut().signal(String::new());
    let id = add(
        &mut h,
        host,
        "t",
        NodeStyle::leaf().width(200.0),
        TextField::new(s, "Name"),
    );
    assert_eq!(h.ui.role(id), Some(Role::TextInput));
    h.focus(id);
    h.type_text("Rock");
    h.settle();
    assert_eq!(s.get(h.ui.rt()), "Rock");
    h.press(KeyCode::Char('z'), Modifiers::CTRL);
    assert_ne!(s.get(h.ui.rt()), "Rock", "field-local undo");
    // IME: pre-edit never reaches the model before Commit.
    let before = s.get(h.ui.rt());
    h.ime(ImeEvent::Preedit {
        text: "ka".into(),
        cursor: Some((2, 2)),
    });
    h.settle();
    assert_eq!(s.get(h.ui.rt()), before);
    h.ime(ImeEvent::Commit("か".into()));
    h.settle();
    assert!(s.get(h.ui.rt()).ends_with('か'));
    h.take::<Submitted>();
    h.press(KeyCode::Enter, NONE);
    assert_eq!(h.take::<Submitted>().len(), 1);
}

#[test]
fn password_field_is_masked_and_never_copied() {
    let (mut h, host) = host();
    h.ui.set_clipboard(Box::new(forge_ui::clipboard::InProcessClipboard::default()));
    let s = h.ui.rt_mut().signal(String::new());
    let id = add(
        &mut h,
        host,
        "p",
        NodeStyle::leaf().width(200.0),
        TextField::new(s, "Password").password(),
    );
    h.focus(id);
    h.type_text("hunter2");
    h.press(KeyCode::Char('a'), Modifiers::CTRL);
    h.press(KeyCode::Char('c'), Modifiers::CTRL);
    assert_eq!(
        h.ui.clipboard().get_text(),
        None,
        "a password is never copied"
    );
    assert_ne!(
        value_of(&mut h, id),
        "hunter2",
        "the a11y value is not the secret"
    );
}

#[test]
fn multiline_editor_edits_lines_with_undo_and_ime() {
    let (mut h, host) = host();
    let s = h.ui.rt_mut().signal(String::from("one"));
    let id = add(
        &mut h,
        host,
        "m",
        NodeStyle::leaf().size(300.0, 120.0),
        MultilineEditor::new(s, "Script").line_numbers(),
    );
    let next = add(&mut h, host, "next", NodeStyle::leaf(), Button::new("Next"));
    assert_eq!(h.ui.role(id), Some(Role::MultilineTextInput));
    h.focus(id);
    h.press(KeyCode::End, Modifiers::CTRL);
    h.press(KeyCode::Enter, NONE);
    h.type_text("two");
    h.settle();
    assert_eq!(s.get(h.ui.rt()), "one\ntwo", "Enter inserts a newline");
    h.press(KeyCode::Up, NONE);
    let c =
        h.ui.widget::<MultilineEditor>(id)
            .map_or(99, |m| m.cursor());
    assert!(c <= 3, "Up moved to the first line (cursor {c})");
    h.press(KeyCode::Char('z'), Modifiers::CTRL);
    assert_ne!(s.get(h.ui.rt()), "one\ntwo");
    h.ime(ImeEvent::Preedit {
        text: "x".into(),
        cursor: None,
    });
    h.settle();
    assert!(!s.get(h.ui.rt()).contains('x'), "pre-edit is not committed");
    // While composing, keys belong to the IME; cancelling the composition hands them back.
    h.ime(ImeEvent::Preedit {
        text: String::new(),
        cursor: None,
    });
    h.settle();
    // Tab leaves (keyboard users are never trapped).
    h.tab(false);
    h.settle();
    assert_eq!(h.ui.focused(), Some(next));
}

#[test]
fn search_field_debounces_and_escape_clears() {
    let (mut h, host) = host();
    let q = h.ui.rt_mut().signal(String::new());
    let id = add(
        &mut h,
        host,
        "s",
        NodeStyle::leaf().width(200.0),
        SearchField::new(q, "Search"),
    );
    assert_eq!(h.ui.role(id), Some(Role::SearchInput));
    h.focus(id);
    h.take::<SearchChanged>();
    h.type_text("tree");
    h.settle();
    assert!(
        h.take::<SearchChanged>().is_empty(),
        "nothing before the debounce"
    );
    h.advance(SEARCH_DEBOUNCE + Duration::from_millis(5));
    let got = h.take::<SearchChanged>();
    assert_eq!(got.len(), 1, "one event per pause");
    assert_eq!(got[0].query, "tree");
    assert_eq!(q.get(h.ui.rt()), "tree");
    h.press(KeyCode::Escape, NONE);
    h.advance(SEARCH_DEBOUNCE + Duration::from_millis(5));
    assert_eq!(q.get(h.ui.rt()), "");
}

#[test]
fn path_field_takes_file_drops_and_enter() {
    let (mut h, host) = host();
    let id = add(
        &mut h,
        host,
        "p",
        NodeStyle::leaf().width(240.0),
        PathField::new("Output", "build"),
    );
    assert_eq!(h.ui.role(id), Some(Role::TextInput));
    let r = h.ui.rect(id).unwrap_or_default();
    h.input(InputEvent::FileDropped {
        pos: r.center(),
        path: "C:/assets/rock.png".into(),
    });
    h.settle();
    assert_eq!(
        h.ui.widget::<PathField>(id)
            .map(|p| p.text().to_string())
            .as_deref(),
        Some("C:/assets/rock.png")
    );
    h.focus(id);
    h.take::<PathChosen>();
    h.press(KeyCode::Enter, NONE);
    assert_eq!(h.take::<PathChosen>().len(), 1);
}

// ---- choice ------------------------------------------------------------------------------

#[test]
fn combo_opens_filters_and_chooses_by_keyboard() {
    let (mut h, host) = host();
    let sel = h.ui.rt_mut().signal(0usize);
    let id = add(
        &mut h,
        host,
        "c",
        NodeStyle::leaf().width(180.0),
        ComboBox::new(
            "Blend",
            &["Opaque", "Masked", "Translucent", "Additive"],
            sel,
        ),
    );
    assert_eq!(h.ui.role(id), Some(Role::ComboBox));
    h.focus(id);
    h.press(KeyCode::Enter, NONE);
    assert!(h.ui.widget::<ComboBox>(id).is_some_and(|c| c.is_open()));
    assert_eq!(h.ui.popups().len(), 1);
    h.type_text("add");
    h.press(KeyCode::Enter, NONE);
    assert_eq!(sel.get(h.ui.rt()), 3, "filtered to Additive and chose it");
    assert!(h.ui.popups().is_empty());
    assert_eq!(h.ui.focused(), Some(id), "focus returns to the combo");
    // Closed: a letter selects the next option starting with it.
    h.type_text("m");
    h.settle();
    assert_eq!(sel.get(h.ui.rt()), 1);
    // Escape closes without choosing.
    h.press(KeyCode::Down, Modifiers { alt: true, ..NONE });
    h.press(KeyCode::Down, NONE);
    h.press(KeyCode::Escape, NONE);
    assert_eq!(sel.get(h.ui.rt()), 1);
    assert!(h.ui.popups().is_empty());
}

#[test]
fn chips_remove_by_keyboard_and_add_from_the_list() {
    let (mut h, host) = host();
    let sel: Signal<Vec<usize>> = h.ui.rt_mut().signal(vec![0, 2]);
    let id = add(
        &mut h,
        host,
        "c",
        NodeStyle::leaf().width(320.0),
        Chips::new("Tags", &["terrain", "water", "foliage", "rock"], sel),
    );
    assert_eq!(h.ui.role(id), Some(Role::List));
    h.focus(id);
    h.press(KeyCode::Home, NONE);
    h.press(KeyCode::Delete, NONE);
    assert_eq!(sel.get(h.ui.rt()), vec![2], "Delete removed the first chip");
    h.press(KeyCode::End, NONE);
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        h.ui.popups().len(),
        1,
        "Enter on the add slot opens the list"
    );
    h.type_text("rock");
    h.press(KeyCode::Enter, NONE);
    assert_eq!(sel.get(h.ui.rt()), vec![2, 3]);
}

#[test]
fn autocomplete_suggests_and_accepts() {
    let (mut h, host) = host();
    let v = h.ui.rt_mut().signal(String::new());
    let id = add(
        &mut h,
        host,
        "a",
        NodeStyle::leaf().width(200.0),
        Autocomplete::new(
            "Component",
            &["Transform", "Rigid body", "Collider", "Light"],
            v,
        ),
    );
    assert_eq!(h.ui.role(id), Some(Role::ComboBox));
    h.focus(id);
    h.type_text("rb");
    h.settle();
    let sug =
        h.ui.widget::<Autocomplete>(id)
            .map(|a| a.suggestions())
            .unwrap_or_default();
    assert_eq!(
        sug.first().map(String::as_str),
        Some("Rigid body"),
        "{sug:?}"
    );
    assert_eq!(h.ui.focused(), Some(id), "focus stays in the entry");
    h.press(KeyCode::Down, NONE);
    h.press(KeyCode::Enter, NONE);
    assert_eq!(v.get(h.ui.rt()), "Rigid body");
}
