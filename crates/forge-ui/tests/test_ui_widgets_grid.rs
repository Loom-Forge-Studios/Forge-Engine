//! The tile grid (the asset browser's body) and row badges (the hierarchy's visibility and
//! lock toggles): virtualised thumbnails — requested only for painted tiles, delivered
//! through the window's poster, evicted beyond the cap — keyboard and pointer model
//! actions, OS file drops, grid/list modes, and badge clicks.
//!
//! `grid_thumbnails_are_virtualised` has a W2 positive control
//! (`positive_control_a_grid_requesting_every_thumbnail_fails`).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use forge_ui::testing::Harness;
use forge_ui::ui::Poster;
use forge_ui::widgets::*;
use forge_ui::{InputEvent, KeyCode, Modifiers, NodeStyle, Point, Signal, WidgetId};

const NONE: Modifiers = Modifiers::NONE;

/// A provider that "renders" at once on request and posts the result.
struct Instant {
    signal: Signal<u64>,
    poster: RefCell<Option<Poster>>,
    inbox: Arc<Mutex<Vec<ReadyThumb>>>,
    requests: Cell<u64>,
}

impl ThumbProvider for Instant {
    fn request(&self, key: u64) {
        self.requests.set(self.requests.get() + 1);
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ReadyThumb {
                key,
                w: 8,
                h: 8,
                rgba: vec![200; 8 * 8 * 4],
            });
        let s = self.signal;
        if let Some(p) = self.poster.borrow().as_ref() {
            p.post(move |rt| s.update(rt, |g| *g += 1));
        }
    }
    fn take_ready(&self) -> Vec<ReadyThumb> {
        std::mem::take(
            &mut *self
                .inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
    fn ready_signal(&self) -> Signal<u64> {
        self.signal
    }
}

fn grid(n: u64, fault: bool) -> (Harness, WidgetId, Rc<Instant>) {
    let mut h = Harness::new(forge_ui::UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let provider = Rc::new(Instant {
        signal: h.ui.rt_mut().signal(0u64),
        poster: RefCell::new(Some(h.ui.poster())),
        inbox: Arc::new(Mutex::new(Vec::new())),
        requests: Cell::new(0),
    });
    let mut g = VirtualGrid::new("Assets").provider(provider.clone());
    if fault {
        g = g.with_fault_request_all();
    }
    g.set_tiles(
        (0..n)
            .map(|i| {
                (
                    i,
                    GridTile::new(format!("asset_{i}.png"), "texture").drag_id(format!("{i:032x}")),
                )
            })
            .collect(),
    );
    let root = h.ui.root();
    let id =
        h.ui.add(root, "grid", NodeStyle::leaf().size(600.0, 400.0), g)
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, id, provider)
}

/// Thumbnails requested and resident for a 100k-tile grid, after scrolling through 40
/// screens. Visible tiles at 600×400 with 96 px tiles: 5 columns × at most 4 rows.
fn check_virtualised(fault: bool) -> Result<(u64, usize), String> {
    let (mut h, id, p) = grid(100_000, fault);
    let r = h.ui.rect(id).unwrap_or_default();
    for _ in 0..40 {
        h.input(InputEvent::Wheel {
            pos: r.center(),
            dx: 0.0,
            dy: -3.0,
        });
        h.settle();
    }
    let resident =
        h.ui.widget::<VirtualGrid>(id)
            .map_or(0, VirtualGrid::resident_thumbs);
    let per_screen = 5 * 4;
    // 40 screens were painted: each asks for at most its visible tiles.
    if p.requests.get() > 41 * per_screen as u64 * 2 {
        return Err(format!(
            "{} thumbnails requested for 40 screens of 20 tiles",
            p.requests.get()
        ));
    }
    if resident > THUMB_CACHE {
        return Err(format!(
            "{resident} thumbnails resident (cap {THUMB_CACHE})"
        ));
    }
    Ok((p.requests.get(), resident))
}

#[test]
fn grid_thumbnails_are_virtualised() {
    let (req, resident) = check_virtualised(false).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        req > 0 && resident > 0,
        "thumbnails arrived: {req} requested, {resident} resident"
    );
}

#[test]
fn positive_control_a_grid_requesting_every_thumbnail_fails() {
    let e = check_virtualised(true).err().unwrap_or_default();
    assert!(
        e.contains("requested") || e.contains("resident"),
        "a grid requesting every thumbnail passed: {e:?}"
    );
}

#[test]
fn thumbnails_arrive_through_the_poster_and_are_drawn() {
    let (mut h, id, p) = grid(12, false);
    h.settle();
    let asked = p.requests.get();
    let t0 = h.ui.widget::<VirtualGrid>(id).and_then(|g| g.thumb_of(0));
    assert!(
        t0.is_some(),
        "the posted thumbnail was uploaded into the atlas"
    );
    assert!(h.ui.image_count() >= 12);
    // Invalidating one (its content changed) frees its image; it is asked for again.
    let freed = VirtualGrid::edit(&mut h.ui, id, |g| g.invalidate_thumb(0)).flatten();
    assert!(freed.is_some());
    h.settle();
    // The provider's post woke the loop: the runner's next turn takes it.
    assert!(h.take_wake(), "a ready thumbnail wakes the loop");
    h.step();
    h.settle();
    assert_eq!(p.requests.get(), asked + 1, "asked for again, once");
    assert!(
        h.ui.widget::<VirtualGrid>(id)
            .and_then(|g| g.thumb_of(0))
            .is_some(),
        "re-rendered"
    );
}

#[test]
fn grid_keyboard_and_pointer_raise_model_actions() {
    let (mut h, id, _p) = grid(30, false);
    h.focus(id);
    h.press(KeyCode::Right, NONE);
    h.press(KeyCode::Down, NONE);
    let sel =
        h.ui.widget::<VirtualGrid>(id)
            .map(VirtualGrid::selected)
            .unwrap_or_default();
    assert_eq!(sel, vec![6], "right then down a row of 5 tiles");
    h.press(KeyCode::Enter, NONE);
    let acts = h.take::<RowActivated>();
    assert_eq!(acts.last().map(|a| a.key), Some(6));
    h.press(KeyCode::F2, NONE);
    assert!(
        h.ui.widget::<VirtualGrid>(id)
            .is_some_and(VirtualGrid::is_renaming)
    );
    h.type_text("wall");
    h.press(KeyCode::Enter, NONE);
    let renamed = h.take::<RowRenamed>();
    assert_eq!(
        renamed.last().map(|r| (r.key, r.name.clone())),
        Some((6, "wall".into()))
    );
    h.press(KeyCode::Delete, NONE);
    assert_eq!(
        h.take::<RowsDeleteRequested>()
            .last()
            .map(|d| d.keys.clone()),
        Some(vec![6])
    );
    h.press(KeyCode::Backspace, NONE);
    assert_eq!(h.take::<GridUp>().len(), 1);
    // List mode: one tile per row.
    VirtualGrid::edit(&mut h.ui, id, |g| g.set_mode(GridMode::List));
    h.settle();
    h.press(KeyCode::Down, NONE);
    let sel =
        h.ui.widget::<VirtualGrid>(id)
            .map(VirtualGrid::selected)
            .unwrap_or_default();
    assert_eq!(sel, vec![7], "down moves one row in list mode");
}

#[test]
fn os_file_drops_raise_files_dropped() {
    let (mut h, id, _p) = grid(3, false);
    let r = h.ui.rect(id).unwrap_or_default();
    let path = PathBuf::from("C:/art/rock.png");
    h.input(InputEvent::FileDropped {
        pos: Point::new(r.x + 300.0, r.y + 300.0),
        path: path.clone(),
    });
    h.settle();
    let d = h.take::<FilesDropped>();
    assert_eq!(d.last().map(|d| d.files.clone()), Some(vec![path]));
}

#[test]
fn row_badges_raise_their_click_and_describe_themselves() {
    let mut v = VirtualTree::tree("Scene");
    v.push(
        None,
        1,
        RowItem::new("Ship")
            .badge(RowBadge {
                on_glyph: "o",
                off_glyph: "-",
                on: true,
                label: "Visible",
            })
            .badge(RowBadge {
                on_glyph: "L",
                off_glyph: "_",
                on: false,
                label: "Locked",
            }),
    );
    let (mut h, id) = Harness::with_widget(v, NodeStyle::leaf().size(300.0, 200.0))
        .unwrap_or_else(|e| panic!("{e}"));
    let r = h.ui.rect(id).unwrap_or_default();
    let right = r.x + r.w - 9.0;
    h.click_at(Point::new(right - BADGE_W - 2.0 + 11.0, r.y + 12.0)); // the second badge
    let clicks = h.take::<RowBadgeClicked>();
    assert_eq!(clicks.last().map(|c| (c.key, c.badge)), Some((1, 1)));
    assert!(
        h.ui.widget::<VirtualTree>(id)
            .is_some_and(|t| t.selected().is_empty()),
        "a badge click does not select the row"
    );
    let node = h.child_node(id, 1).unwrap_or_else(|| panic!("row node"));
    assert_eq!(node.label(), Some("Ship (Visible on, Locked off)"));
}
