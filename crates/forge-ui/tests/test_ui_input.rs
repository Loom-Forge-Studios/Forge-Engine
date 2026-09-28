//! Input behaviours of Ch.21 §21.4, §21.9 and §21.14 not covered by the named guards:
//! pointer capture, hover enter/leave on change only, clipboard copy/cut/paste (copy is a
//! read), typed drag-and-drop with verdicts and OS file drops, keyed reorder keeping
//! focus, stale-handle detection, DPI scaling (layout logical, text re-shaped), and
//! windows sharing one atlas and shaping cache.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use forge_ui::dnd::{DragPayload, DropVerdict};
use forge_ui::gallery::{self, GalleryFaults};
use forge_ui::testing::Harness;
use forge_ui::widget::{EventCx, PaintCx};
use forge_ui::widgets::{Button, Container, Label, Pressed};
use forge_ui::{
    Handled, InputEvent, Key, KeyCode, Modifiers, NodeStyle, Point, PointerButton, Role, Size, Ui,
    UiConfig, UiEvent, Widget,
};

fn harness() -> (Harness, gallery::Gallery) {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, GalleryFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, g)
}

#[test]
fn slider_drag_keeps_capture_outside_its_rect() {
    let (mut h, g) = harness();
    let r = h.ui.rect(g.slider).unwrap_or_default();
    let down = Point::new(r.x + 4.0, r.center().y);
    h.input(InputEvent::PointerMoved(down));
    h.input(InputEvent::PointerButton {
        pos: down,
        button: PointerButton::Primary,
        pressed: true,
    });
    // Drag far past the right edge and below the widget: still the slider's gesture.
    h.input(InputEvent::PointerMoved(Point::new(
        r.right() + 300.0,
        r.bottom() + 200.0,
    )));
    assert_eq!(g.opacity.get(h.ui.rt()), 1.0);
    h.input(InputEvent::PointerButton {
        pos: Point::new(r.right() + 300.0, r.bottom() + 200.0),
        button: PointerButton::Primary,
        pressed: false,
    });
    h.input(InputEvent::PointerMoved(Point::new(r.x, r.center().y)));
    assert_eq!(g.opacity.get(h.ui.rt()), 1.0, "capture ended at release");
}

#[test]
fn click_raises_pressed_only_when_released_inside() {
    let (mut h, g) = harness();
    h.click(g.save);
    let got: Vec<_> =
        h.ui.take_actions()
            .iter()
            .filter_map(|a| a.get::<Pressed>().copied())
            .collect();
    assert_eq!(got, vec![Pressed(g.save)]);
    let r = h.ui.rect(g.save).unwrap_or_default();
    h.input(InputEvent::PointerButton {
        pos: r.center(),
        button: PointerButton::Primary,
        pressed: true,
    });
    h.input(InputEvent::PointerButton {
        pos: Point::new(r.x - 50.0, r.y - 50.0),
        button: PointerButton::Primary,
        pressed: false,
    });
    assert!(
        h.ui.take_actions()
            .iter()
            .all(|a| a.get::<Pressed>().is_none()),
        "released outside: no press"
    );
}

/// Counts enter/leave events it receives.
struct HoverProbe(Rc<RefCell<Vec<&'static str>>>);
impl Widget for HoverProbe {
    fn role(&self) -> Role {
        Role::Group
    }
    fn event(&mut self, _cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PointerEnter => self.0.borrow_mut().push("enter"),
            UiEvent::PointerLeave => self.0.borrow_mut().push("leave"),
            _ => {}
        }
        Handled::No
    }
    fn paint(&self, _cx: &mut PaintCx) {}
}

#[test]
fn hover_enter_and_leave_fire_only_on_change() {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let log = Rc::new(RefCell::new(Vec::new()));
    let id =
        h.ui.add(
            h.ui.root(),
            "probe",
            NodeStyle::leaf().size(100.0, 100.0),
            HoverProbe(log.clone()),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let r = h.ui.rect(id).unwrap_or_default();
    for dx in 0..5 {
        h.move_to(Point::new(r.x + 10.0 + dx as f32, r.y + 10.0));
    }
    h.move_to(Point::new(r.right() + 50.0, r.y));
    h.move_to(Point::new(r.right() + 60.0, r.y));
    assert_eq!(*log.borrow(), vec!["enter", "leave"]);
}

#[test]
fn clipboard_copy_cut_paste_between_fields() {
    let (mut h, g) = harness();
    h.ui.set_focus(Some(g.text_field), true);
    h.type_text("hello world");
    h.key(KeyCode::Char('a'), Modifiers::CTRL);
    h.key(KeyCode::Char('c'), Modifiers::CTRL);
    assert_eq!(g.name.get(h.ui.rt()), "hello world", "copy is a read");
    assert_eq!(h.ui.clipboard().get_text().as_deref(), Some("hello world"));
    h.key(KeyCode::Char('x'), Modifiers::CTRL);
    assert_eq!(g.name.get(h.ui.rt()), "");
    h.key(KeyCode::Char('v'), Modifiers::CTRL);
    h.key(KeyCode::Char('v'), Modifiers::CTRL);
    assert_eq!(g.name.get(h.ui.rt()), "hello worldhello world");
    assert!(
        h.ui.clipboard().backend_name().contains("in-process"),
        "the D-4 backend is labelled"
    );
}

#[test]
fn grapheme_editing_and_selection() {
    let (mut h, g) = harness();
    h.ui.set_focus(Some(g.text_field), true);
    let mut ev = forge_ui::KeyEvent::press(KeyCode::Other, Modifiers::NONE);
    ev.text = Some("e\u{301}🇳🇱x".into()); // e + combining acute, a flag (two code points)
    h.input(InputEvent::Key(ev));
    h.key(KeyCode::Backspace, Modifiers::NONE);
    h.key(KeyCode::Backspace, Modifiers::NONE);
    assert_eq!(
        g.name.get(h.ui.rt()),
        "e\u{301}",
        "backspace removes whole graphemes"
    );
    h.key(KeyCode::Left, Modifiers::NONE);
    assert_eq!(
        h.ui.widget::<forge_ui::widgets::TextField>(g.text_field)
            .map(|f| f.cursor()),
        Some(0)
    );
    h.key(KeyCode::End, Modifiers::SHIFT);
    h.key(KeyCode::Delete, Modifiers::NONE);
    assert_eq!(g.name.get(h.ui.rt()), "");
}

/// A drag source that starts a drag of an asset on press.
struct Source;
impl Widget for Source {
    fn role(&self) -> Role {
        Role::Button
    }
    fn focusable(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if let UiEvent::PointerDown { .. } = ev {
            cx.start_drag(DragPayload::Asset("rock.mesh".into()));
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &forge_ui::widget::A11yCx, node: &mut accesskit::Node) {
        node.set_label("Drag source");
    }
}

/// A drop target accepting assets only; records drops.
struct Target(Rc<RefCell<Vec<DragPayload>>>);
impl Widget for Target {
    fn role(&self) -> Role {
        Role::Group
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::DragOver(p) => {
                cx.set_drop_verdict(match p {
                    DragPayload::Asset(_) | DragPayload::Files(_) => DropVerdict::Accepted,
                    _ => DropVerdict::Refused("only assets".into()),
                });
                Handled::Yes
            }
            UiEvent::Drop(p) => {
                self.0.borrow_mut().push(p.clone());
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut PaintCx) {}
}

#[test]
fn typed_drag_and_drop_with_verdicts_and_os_file_drops() {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let row =
        h.ui.add(h.ui.root(), "row", NodeStyle::row(40.0), Container::group())
            .unwrap_or_else(|e| panic!("{e}"));
    let src =
        h.ui.add(row, "src", NodeStyle::leaf().size(80.0, 40.0), Source)
            .unwrap_or_else(|e| panic!("{e}"));
    let dst =
        h.ui.add(
            row,
            "dst",
            NodeStyle::leaf().size(120.0, 80.0),
            Target(dropped.clone()),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let (s, d) = (
        h.ui.rect(src).unwrap_or_default(),
        h.ui.rect(dst).unwrap_or_default(),
    );
    h.input(InputEvent::PointerMoved(s.center()));
    h.input(InputEvent::PointerButton {
        pos: s.center(),
        button: PointerButton::Primary,
        pressed: true,
    });
    h.input(InputEvent::PointerMoved(d.center()));
    assert_eq!(
        h.ui.drag().and_then(|x| x.over.clone()),
        Some((dst, DropVerdict::Accepted))
    );
    h.input(InputEvent::PointerButton {
        pos: d.center(),
        button: PointerButton::Primary,
        pressed: false,
    });
    assert_eq!(
        *dropped.borrow(),
        vec![DragPayload::Asset("rock.mesh".into())]
    );
    assert!(h.ui.drag().is_none());
    // An OS file drop arrives as a typed Files payload.
    h.input(InputEvent::FileDropped {
        pos: d.center(),
        path: "C:/tmp/a.png".into(),
    });
    assert_eq!(dropped.borrow().len(), 2);
    assert!(matches!(&dropped.borrow()[1], DragPayload::Files(f) if f.len() == 1));
}

#[test]
fn keyed_reorder_moves_widgets_and_keeps_focus() {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let list =
        h.ui.add(
            h.ui.root(),
            "list",
            NodeStyle::column(4.0),
            Container::group(),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let ids: Vec<_> = (0..3u64)
        .map(|i| {
            h.ui.add(
                list,
                Key::Id(1000 + i),
                NodeStyle::leaf(),
                Button::new(format!("Row {i}")),
            )
            .unwrap_or_else(|e| panic!("{e}"))
        })
        .collect();
    h.settle();
    h.ui.set_focus(Some(ids[0]), true);
    let y0 = h.ui.rect(ids[0]).unwrap_or_default().y;
    h.ui.reorder(list, &[Key::Id(1002), Key::Id(1001), Key::Id(1000)])
        .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    assert_eq!(h.ui.children(list), vec![ids[2], ids[1], ids[0]]);
    assert_eq!(
        h.ui.focused(),
        Some(ids[0]),
        "focus survives a keyed reorder"
    );
    assert!(
        h.ui.rect(ids[0]).unwrap_or_default().y > y0,
        "the widget moved, it was not recreated"
    );
    // Ids are stable across runs: the same key under the same parent is the same id.
    assert_eq!(ids[1], list.child(&Key::Id(1001)));
}

#[test]
fn removed_widgets_are_stale_not_aliased() {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let a =
        h.ui.add(h.ui.root(), "a", NodeStyle::leaf(), Label::new("A"))
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    h.ui.remove(a).unwrap_or_else(|e| panic!("{e}"));
    assert!(h.ui.widget::<Label>(a).is_none());
    assert!(matches!(
        h.ui.remove(a),
        Err(forge_ui::UiError::UnknownWidget(_))
    ));
    let b =
        h.ui.add(h.ui.root(), "b", NodeStyle::leaf(), Label::new("B"))
            .unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(a, b);
    assert!(matches!(
        h.ui.add(h.ui.root(), "b", NodeStyle::leaf(), Label::new("B again")),
        Err(forge_ui::UiError::DuplicateKey(_))
    ));
}

#[test]
fn scale_change_reshapes_text_but_keeps_logical_layout() {
    let (mut h, g) = harness();
    let before = h.ui.rect(g.save);
    let shapes = h.ui.text().stats.shape_calls;
    let hash = h.ui.display_list_hash();
    h.ui.set_viewport(h.ui.size(), 1.5);
    h.settle();
    assert_eq!(
        h.ui.rect(g.save),
        before,
        "layout is logical; a scale change does not move widgets"
    );
    assert!(
        h.ui.text().stats.shape_calls > shapes,
        "text re-shapes at the new size"
    );
    assert_ne!(h.ui.display_list_hash(), hash);
    // Physical snapping: at 150% a 1-px border lands on whole device pixels.
    let s = h.full_frame().unwrap_or_default();
    assert!(s.damage_px >= (1280.0f64 * 1.5 * 800.0 * 1.5 * 0.99) as u64);
}

#[test]
fn windows_share_one_atlas_and_one_shaping_cache() {
    let mut a = Ui::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    a.add(
        a.root(),
        "l",
        NodeStyle::leaf(),
        Label::new("Shared text in two windows"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    a.frame(Duration::ZERO);
    let res = a.resources();
    let mut b = Ui::with_resources(
        UiConfig {
            size: Size::new(400.0, 300.0),
            ..UiConfig::default()
        },
        res.clone(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let before = b.text().stats.shape_calls;
    b.add(
        b.root(),
        "l",
        NodeStyle::leaf(),
        Label::new("Shared text in two windows"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    b.frame(Duration::ZERO);
    assert_eq!(
        b.text().stats.shape_calls,
        before,
        "the second window reused the first window's shaping"
    );
    assert_eq!(a.text().atlas_count(), 1);
    assert!(Rc::ptr_eq(&a.resources(), &b.resources()));
    // Different DPI per window: each shapes at its own scale through the shared cache.
    b.set_viewport(b.size(), 2.0);
    b.frame(Duration::ZERO);
    assert!(b.text().stats.shape_calls > before);
}

#[test]
fn reduced_motion_disables_the_caret_blink() {
    let (mut h, g) = harness();
    h.ui.set_reduced_motion(true);
    h.ui.set_focus(Some(g.text_field), true);
    h.type_text("x");
    h.settle();
    let frames = h.advance(Duration::from_secs(3));
    assert_eq!(frames, 0, "reduced motion: a solid caret, no blink frames");
}
