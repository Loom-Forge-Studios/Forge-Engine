//! Catalogue widget tests, part 3 (§21.12, §21.16; DoD M2-24/M2-25): the virtualised
//! list, tree and table — multi-select, type-ahead, rename in place, drag-drop and
//! keyboard reorder, expand/collapse, sort and resize, and their accessibility trees.
//! The 100k budgets are `tests/perf/test_ui_budgets.rs::ui_virtual_*_100k`.

use std::time::Duration;

use forge_ui::testing::Harness;
use forge_ui::widgets::*;
use forge_ui::{ImeEvent, KeyCode, Modifiers, NodeStyle, Point, Role, WidgetId};

const NONE: Modifiers = Modifiers::NONE;
const SHIFT: Modifiers = Modifiers::SHIFT;
const CTRL: Modifiers = Modifiers::CTRL;
fn alt() -> Modifiers {
    Modifiers { alt: true, ..NONE }
}

fn list(n: u64) -> (Harness, WidgetId) {
    let mut v = VirtualTree::list("Entities");
    v.extend((0..n).map(|i| (i, RowItem::new(format!("Entity {i}")))));
    let (mut h, id) = Harness::with_widget(v, NodeStyle::leaf().size(260.0, 240.0))
        .unwrap_or_else(|e| panic!("{e}"));
    h.focus(id);
    (h, id)
}
fn selected(h: &Harness, id: WidgetId) -> Vec<u64> {
    h.ui.widget::<VirtualTree>(id)
        .map(|t| t.selected())
        .unwrap_or_default()
}
fn row_point(h: &Harness, id: WidgetId, row: usize, frac: f32) -> Point {
    let r = h.ui.rect(id).unwrap_or_default();
    Point::new(r.x + 60.0, r.y + (row as f32 + frac) * 24.0)
}

#[test]
fn list_role_rows_and_live_accessibility_nodes() {
    let (mut h, id) = list(1000);
    assert_eq!(h.ui.role(id), Some(Role::ListBox));
    let row0 = h
        .child_node(id, 0)
        .unwrap_or_else(|| panic!("row 0 has a node"));
    assert_eq!(row0.role(), Role::ListBoxOption);
    assert_eq!(row0.label(), Some("Entity 0"));
    assert_eq!(row0.size_of_set(), Some(1000));
    assert!(
        h.child_node(id, 900).is_none(),
        "off-screen rows have no node (virtualised)"
    );
    let live = h.ui.widget::<VirtualTree>(id).map_or(0, |t| t.live_rows());
    assert!(
        live <= (240.0f32 / 24.0).ceil() as usize + 16,
        "{live} live rows"
    );
}

#[test]
fn list_keyboard_selection_single_range_toggle_all() {
    let (mut h, id) = list(50);
    h.press(KeyCode::Down, NONE);
    assert_eq!(
        selected(&h, id),
        vec![1],
        "focus starts on the top row; Down selects the next"
    );
    h.press(KeyCode::Down, SHIFT);
    h.press(KeyCode::Down, SHIFT);
    assert_eq!(
        selected(&h, id),
        vec![1, 2, 3],
        "Shift extends from the anchor"
    );
    h.press(KeyCode::Down, CTRL);
    h.press(KeyCode::Down, CTRL);
    h.press(KeyCode::Space, CTRL);
    assert_eq!(
        selected(&h, id),
        vec![1, 2, 3, 5],
        "Ctrl moves without selecting; Ctrl+Space toggles"
    );
    h.press(KeyCode::Char('a'), CTRL);
    assert_eq!(selected(&h, id).len(), 50);
    h.press(KeyCode::End, NONE);
    assert_eq!(selected(&h, id), vec![49]);
    let active = h.ui.widget::<VirtualTree>(id).and_then(|t| t.active());
    assert_eq!(active, Some(49));
    assert!(
        h.ui.widget::<VirtualTree>(id)
            .is_some_and(|t| t.scroll_offset() > 0.0),
        "End scrolls the row into view"
    );
    assert!(
        h.child_node(id, 49)
            .is_some_and(|n| n.is_selected() == Some(true))
    );
    assert!(!h.take::<SelectionChanged>().is_empty());
}

#[test]
fn list_pointer_selection_with_modifiers() {
    let (mut h, id) = list(20);
    h.click_at(row_point(&h, id, 2, 0.5));
    assert_eq!(selected(&h, id), vec![2]);
    h.input(forge_ui::InputEvent::Modifiers(SHIFT));
    h.click_at(row_point(&h, id, 4, 0.5));
    assert_eq!(selected(&h, id), vec![2, 3, 4]);
    h.input(forge_ui::InputEvent::Modifiers(CTRL));
    h.click_at(row_point(&h, id, 3, 0.5));
    assert_eq!(selected(&h, id), vec![2, 4]);
    h.input(forge_ui::InputEvent::Modifiers(NONE));
    h.take::<RowActivated>();
    let p = row_point(&h, id, 7, 0.5);
    h.click_at(p);
    h.click_at(p);
    assert_eq!(
        h.take::<RowActivated>(),
        vec![RowActivated { view: id, key: 7 }],
        "double click activates"
    );
}

#[test]
fn list_type_ahead_jumps_and_resets() {
    let mut v = VirtualTree::list("Fruit");
    for (i, n) in ["apple", "banana", "blueberry", "cherry", "citrus"]
        .iter()
        .enumerate()
    {
        v.push(None, i as u64, RowItem::new(*n));
    }
    let (mut h, id) = Harness::with_widget(v, NodeStyle::leaf().size(200.0, 200.0))
        .unwrap_or_else(|e| panic!("{e}"));
    h.focus(id);
    h.type_text("bl");
    h.settle();
    assert_eq!(selected(&h, id), vec![2], "\"bl\" finds blueberry");
    h.advance(TYPEAHEAD_RESET + Duration::from_millis(10));
    h.type_text("c");
    h.settle();
    assert_eq!(
        selected(&h, id),
        vec![3],
        "after the reset, \"c\" starts a new search"
    );
    h.type_text("c");
    h.advance(TYPEAHEAD_RESET + Duration::from_millis(10));
    h.type_text("c");
    h.settle();
    assert_eq!(selected(&h, id), vec![4], "repeating a letter cycles");
}

#[test]
fn rename_in_place_commits_cancels_and_uses_the_ime() {
    let (mut h, id) = list(5);
    h.press(KeyCode::Space, NONE);
    h.press(KeyCode::F2, NONE);
    assert!(
        h.ui.widget::<VirtualTree>(id)
            .is_some_and(|t| t.is_renaming())
    );
    h.type_text("Rock");
    h.ime(ImeEvent::Preedit {
        text: "ish".into(),
        cursor: None,
    });
    h.settle();
    assert!(
        h.take::<RowRenamed>().is_empty(),
        "nothing is renamed while typing"
    );
    h.ime(ImeEvent::Commit("y".into()));
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        h.take::<RowRenamed>(),
        vec![RowRenamed {
            view: id,
            key: 0,
            name: "Rocky".into()
        }]
    );
    // I7: the view did not rename its model itself; the owner does.
    assert_eq!(
        h.ui.widget::<VirtualTree>(id)
            .and_then(|t| t.label_of(0).map(str::to_string))
            .as_deref(),
        Some("Entity 0")
    );
    h.press(KeyCode::F2, NONE);
    h.type_text("zzz");
    h.press(KeyCode::Escape, NONE);
    assert!(h.take::<RowRenamed>().is_empty(), "Escape cancels");
    assert!(
        !h.ui
            .widget::<VirtualTree>(id)
            .is_some_and(|t| t.is_renaming())
    );
}

#[test]
fn delete_and_keyboard_reorder_raise_model_actions() {
    let (mut h, id) = list(5);
    h.press(KeyCode::Down, NONE);
    h.press(KeyCode::Down, alt());
    let d = h.take::<RowsDropped>();
    assert_eq!(
        d,
        vec![RowsDropped {
            view: id,
            keys: vec![1],
            target: DropTarget {
                parent: None,
                index: 3
            }
        }]
    );
    h.press(KeyCode::Up, alt());
    assert_eq!(h.take::<RowsDropped>()[0].target.index, 0);
    h.press(KeyCode::Delete, NONE);
    assert_eq!(
        h.take::<RowsDeleteRequested>(),
        vec![RowsDeleteRequested {
            view: id,
            keys: vec![1]
        }]
    );
    h.press(KeyCode::ContextMenu, NONE);
    assert_eq!(h.take::<RowContextMenu>().len(), 1);
}

#[test]
fn drag_and_drop_reorders_with_an_insertion_target() {
    let (mut h, id) = list(10);
    // Drag row 1 onto the lower half of row 5: insert after it.
    h.drag(row_point(&h, id, 1, 0.5), row_point(&h, id, 5, 0.8), 8);
    let d = h.take::<RowsDropped>();
    assert_eq!(
        d,
        vec![RowsDropped {
            view: id,
            keys: vec![1],
            target: DropTarget {
                parent: None,
                index: 6
            }
        }],
        "{d:?}"
    );
}

fn tree() -> (Harness, WidgetId) {
    let mut t = VirtualTree::tree("Scene");
    for f in 0..3u64 {
        let fk = 100 * (f + 1);
        t.push(None, fk, RowItem::new(format!("Folder {f}")));
        for c in 1..=3u64 {
            t.push(Some(fk), fk + c, RowItem::new(format!("Node {f}.{c}")));
        }
    }
    let (mut h, id) = Harness::with_widget(t, NodeStyle::leaf().size(260.0, 300.0))
        .unwrap_or_else(|e| panic!("{e}"));
    h.focus(id);
    (h, id)
}

#[test]
fn tree_expands_collapses_and_walks_by_keyboard() {
    let (mut h, id) = tree();
    assert_eq!(h.ui.role(id), Some(Role::Tree));
    let rows = |h: &Harness| h.ui.widget::<VirtualTree>(id).map_or(0, |t| t.row_count());
    assert_eq!(rows(&h), 3);
    // Focus starts on Folder 0.
    h.press(KeyCode::Right, NONE);
    assert_eq!(rows(&h), 6, "Right expands");
    let n = h
        .child_node(id, 100)
        .unwrap_or_else(|| panic!("folder node"));
    assert_eq!(n.role(), Role::TreeItem);
    assert_eq!(n.is_expanded(), Some(true));
    assert_eq!(n.level(), Some(1));
    h.press(KeyCode::Right, NONE);
    assert_eq!(
        h.ui.widget::<VirtualTree>(id).and_then(|t| t.active()),
        Some(101),
        "Right again enters the first child"
    );
    assert_eq!(h.child_node(id, 101).and_then(|n| n.level()), Some(2));
    h.press(KeyCode::Left, NONE);
    assert_eq!(
        h.ui.widget::<VirtualTree>(id).and_then(|t| t.active()),
        Some(100),
        "Left goes to the parent"
    );
    h.press(KeyCode::Left, NONE);
    assert_eq!(rows(&h), 3, "Left collapses");
    // The disclosure triangle toggles with the pointer.
    let r = h.ui.rect(id).unwrap_or_default();
    h.click_at(Point::new(r.x + 12.0, r.y + 12.0 + 24.0));
    assert_eq!(rows(&h), 6, "clicking Folder 1's triangle expanded it");
}

#[test]
fn tree_drop_into_a_folder_and_refuses_a_cycle() {
    let (mut h, id) = tree();
    VirtualTree::edit(&mut h.ui, id, |t| {
        t.set_expanded(100, true);
        t.set_expanded(200, true);
    });
    h.settle();
    // Rows: 0 Folder0, 1..3 its nodes, 4 Folder1, 5..7, 8 Folder2.
    h.drag(row_point(&h, id, 1, 0.5), row_point(&h, id, 8, 0.5), 8);
    let d = h.take::<RowsDropped>();
    assert_eq!(d.len(), 1);
    assert_eq!(
        d[0].target,
        DropTarget {
            parent: Some(300),
            index: 3
        },
        "the middle of a row drops into it"
    );
    // A folder cannot be dropped into its own child.
    h.advance(Duration::from_millis(600));
    h.drag(row_point(&h, id, 0, 0.5), row_point(&h, id, 2, 0.5), 8);
    assert!(
        h.take::<RowsDropped>().is_empty(),
        "no drop into its own subtree"
    );
}

struct Model {
    order: Vec<u32>,
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
        if col == 0 {
            format!("row {i}")
        } else {
            ((i * 7) % 10).to_string()
        }
    }
    fn sort(&mut self, col: usize, dir: SortDir) {
        if col == 0 {
            self.order.sort_unstable();
        } else {
            self.order.sort_by_key(|i| ((i * 7) % 10, *i));
        }
        if dir == SortDir::Descending {
            self.order.reverse();
        }
    }
}

fn table() -> (Harness, WidgetId) {
    let t = VirtualTable::new(
        "Stats",
        vec![
            Column::new("Name", 120.0),
            Column::new("Score", 80.0).numeric(),
        ],
        Box::new(Model {
            order: (0..100).collect(),
        }),
    );
    let (mut h, id) = Harness::with_widget(t, NodeStyle::leaf().size(260.0, 300.0))
        .unwrap_or_else(|e| panic!("{e}"));
    h.focus(id);
    (h, id)
}

#[test]
fn table_sorts_by_header_click_and_keyboard_and_keeps_the_selection() {
    let (mut h, id) = table();
    assert_eq!(h.ui.role(id), Some(Role::Grid));
    h.press(KeyCode::Down, NONE);
    let sel =
        h.ui.widget::<VirtualTable>(id)
            .map(|t| t.selected())
            .unwrap_or_default();
    assert_eq!(sel, vec![1]);
    // Click the Score header.
    let r = h.ui.rect(id).unwrap_or_default();
    h.click_at(Point::new(r.x + 160.0, r.y + 12.0));
    let t =
        h.ui.widget::<VirtualTable>(id)
            .unwrap_or_else(|| panic!("table"));
    assert_eq!(t.sort_state(), Some((1, SortDir::Ascending)));
    assert_eq!(t.model().cell(0, 1), "0");
    assert_eq!(
        t.selected(),
        vec![1],
        "the selection is kept by key across a sort"
    );
    let active_key = t.active().map(|(r, _)| t.model().key(r));
    assert_eq!(active_key, Some(1), "the active row follows its record");
    assert_eq!(h.take::<TableSorted>().len(), 1);
    // Ctrl+Enter on the same column reverses.
    h.press(KeyCode::Right, NONE);
    h.press(KeyCode::Enter, CTRL);
    assert_eq!(
        h.ui.widget::<VirtualTable>(id).and_then(|t| t.sort_state()),
        Some((1, SortDir::Descending))
    );
    let hdr = h
        .child_node(id, HEADER_KEY_BASE + 1)
        .unwrap_or_else(|| panic!("header node"));
    assert_eq!(hdr.role(), Role::ColumnHeader);
    assert_eq!(
        hdr.sort_direction(),
        Some(accesskit::SortDirection::Descending)
    );
}

#[test]
fn table_columns_resize_by_keyboard_and_drag() {
    let (mut h, id) = table();
    h.press(KeyCode::Right, alt());
    assert_eq!(
        h.ui.widget::<VirtualTable>(id)
            .map(|t| t.columns()[0].width),
        Some(128.0)
    );
    h.press(
        KeyCode::Left,
        Modifiers {
            alt: true,
            shift: true,
            ..NONE
        },
    );
    assert_eq!(
        h.ui.widget::<VirtualTable>(id)
            .map(|t| t.columns()[0].width),
        Some(127.0)
    );
    let r = h.ui.rect(id).unwrap_or_default();
    let edge = Point::new(r.x + 127.0, r.y + 12.0);
    h.drag(edge, Point::new(edge.x + 40.0, edge.y), 4);
    assert_eq!(
        h.ui.widget::<VirtualTable>(id)
            .map(|t| t.columns()[0].width),
        Some(167.0)
    );
    let rs = h.take::<ColumnResized>();
    assert!(rs.iter().any(|c| c.col == 0 && c.width == 167.0), "{rs:?}");
    // Never below the minimum.
    for _ in 0..40 {
        h.press(KeyCode::Left, alt());
    }
    assert_eq!(
        h.ui.widget::<VirtualTable>(id)
            .map(|t| t.columns()[0].width),
        Some(32.0)
    );
}

#[test]
fn table_rows_are_virtual_accessibility_rows() {
    let (mut h, id) = table();
    let row = h.child_node(id, 0).unwrap_or_else(|| panic!("row node"));
    assert_eq!(row.role(), Role::Row);
    assert!(row.label().is_some_and(|l| l.contains("row 0")));
    assert!(
        h.child_node(id, 99).is_none(),
        "off-screen rows are not realised"
    );
    assert_eq!(h.node(id).and_then(|n| n.row_count()), Some(100));
    h.press(KeyCode::End, NONE);
    assert!(
        h.child_node(id, 99).is_some(),
        "scrolling realises the rows it shows"
    );
    h.press(KeyCode::Enter, NONE);
    assert_eq!(
        h.take::<RowActivated>(),
        vec![RowActivated { view: id, key: 99 }]
    );
}
