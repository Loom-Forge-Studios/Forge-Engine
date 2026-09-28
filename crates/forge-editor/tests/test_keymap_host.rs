//! The keymap in a real window (Ch.21 §21.17): keys the focused widget does not handle
//! resolve through the focus path's contexts — the focused widget's role, the dock panel
//! it is in, then window and global — to actions the shell runs.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::keymap::{KeyMap, default_keymap};
use forge_editor::keys::{ActionTriggered, ChordPending, keymap_host};
use forge_editor::panels::{EditorPanelHost, PanelCatalog};
use forge_plugin::points::PresetKind;
use forge_ui::dock::{
    AreaId, Axis, DockNode, DockTabStrip, DockTree, PanelId, dock_area, panel_frame_id,
};
use forge_ui::testing::Harness;
use forge_ui::{Key, KeyCode, Modifiers, NodeStyle, Size, UiConfig, WidgetId};

struct Win {
    h: Harness,
    dock: WidgetId,
}

fn window() -> Win {
    let (keymap, errs) =
        KeyMap::from_layers(&default_keymap().unwrap_or_else(|e| panic!("{e}")), &[]);
    assert!(errs.is_empty(), "{errs:?}");
    let mut h = Harness::new(UiConfig {
        size: Size::new(1200.0, 760.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let root = h.ui.root();
    let host = keymap_host(
        &mut h.ui,
        root,
        "keys",
        NodeStyle::default().fill(),
        Rc::new(RefCell::new(keymap)),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let x = common::load(&[]);
    let tree = h.ui.rt_mut().signal(DockTree::new(DockNode::split(
        Axis::Horizontal,
        vec![
            (0.3, DockNode::tabs(&["forge.hierarchy"])),
            (0.4, DockNode::tabs(&["forge.console"])),
            (0.3, DockNode::tabs(&["forge.assets"])),
        ],
    )));
    let dock = dock_area(
        &mut h.ui,
        host,
        "dock",
        NodeStyle::default().fill(),
        AreaId::Main,
        tree,
        Box::new(EditorPanelHost::new(
            common::panels(&x),
            PresetKind::ThreeD,
            PanelCatalog::default(),
        )),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let _ = h.take::<ActionTriggered>();
    Win { h, dock }
}

fn ctrl(c: char) -> (KeyCode, Modifiers) {
    (KeyCode::Char(c), Modifiers::CTRL)
}

impl Win {
    fn strip_in(&self, panel: &str) -> WidgetId {
        let a = self
            .h
            .ui
            .widget::<forge_ui::dock::DockArea>(self.dock)
            .unwrap_or_else(|| panic!("no dock"));
        let gi = a
            .geometry()
            .groups
            .iter()
            .position(|g| g.panels.contains(&PanelId::new(panel)))
            .unwrap_or(0);
        a.strips()[gi]
    }
    fn press(&mut self, (code, mods): (KeyCode, Modifiers)) -> Vec<String> {
        self.h.press(code, mods);
        self.h
            .take::<ActionTriggered>()
            .into_iter()
            .map(|a| a.action)
            .collect()
    }
}

#[test]
fn a_global_chord_works_with_nothing_focused() {
    let mut w = window();
    assert_eq!(w.h.ui.focused(), None);
    assert_eq!(
        w.press((
            KeyCode::Char('p'),
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::NONE
            }
        )),
        vec!["forge.palette.open".to_string()]
    );
}

#[test]
fn panel_chords_apply_only_inside_their_panel() {
    let mut w = window();
    // Focus inside the console panel (its tab strip belongs to the group, not the frame, so
    // focus a widget in the frame: the stand-in's empty state is not focusable; use F6).
    let console = panel_frame_id(w.dock, &PanelId::new("forge.console"));
    assert!(w.h.ui.contains(console.child(&Key::Static("title"))));
    // With the console's strip focused the panel context is the console's.
    let s = w.strip_in("forge.console");
    w.h.focus(s);
    assert_eq!(w.press(ctrl('l')), vec!["forge.console.clear".to_string()]);
    // F2 means rename-entity in the hierarchy and rename-asset in the assets panel.
    let s = w.strip_in("forge.hierarchy");
    w.h.focus(s);
    assert_eq!(
        w.press((KeyCode::F2, Modifiers::NONE)),
        vec!["forge.hierarchy.rename".to_string()]
    );
    assert!(
        w.press(ctrl('l')).is_empty(),
        "Ctrl+L is the console's, not the hierarchy's"
    );
    let s = w.strip_in("forge.assets");
    w.h.focus(s);
    assert_eq!(
        w.press((KeyCode::F2, Modifiers::NONE)),
        vec!["forge.assets.rename".to_string()]
    );
    // Window chords still apply inside a panel.
    assert_eq!(w.press(ctrl('z')), vec!["forge.edit.undo".to_string()]);
    let _ = w.h.ui.widget::<DockTabStrip>(s).map(DockTabStrip::active);
}

#[test]
fn a_two_stroke_chord_waits_for_its_second_stroke() {
    let mut w = window();
    assert!(w.press(ctrl('k')).is_empty());
    assert_eq!(
        w.h.take::<ChordPending>(),
        Vec::<ChordPending>::new(),
        "taken by press() already"
    );
    assert_eq!(w.press(ctrl('s')), vec!["forge.layout.save_as".to_string()]);
    // Ctrl+S alone is save.
    assert_eq!(w.press(ctrl('s')), vec!["forge.file.save".to_string()]);
    // A first stroke followed by an unbound second stroke runs nothing.
    assert!(w.press(ctrl('k')).is_empty());
    assert!(w.press(ctrl('q')).is_empty());
    assert_eq!(w.press(ctrl('s')), vec!["forge.file.save".to_string()]);
}
