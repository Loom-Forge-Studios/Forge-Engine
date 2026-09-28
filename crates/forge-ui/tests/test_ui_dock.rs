// timed-gates: exempt(the splitter-drag frame time is printed; the asserts are on rebuilds and re-recorded slices)
//! Docking interaction tests (Ch.21 §21.17; DoD M2-26): drag-to-dock with drop previews,
//! tab reorder, tear-off into a floating window, splitter drags, maximise, keyboard
//! operation, unknown panels kept, and one layout across windows.
//!
//! Positive controls (W2):
//! * `positive_control_a_rebuilding_dock_fails_the_state_check` — a dock that rebuilds
//!   panels on every change (what a naive dock does) loses panel state, and the re-dock
//!   check that the real dock passes fails on it;
//! * `positive_control_a_dock_ignoring_drops_fails_the_drag_check` — the drag-to-dock check
//!   fails against a dock area that ignores drops.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use forge_ui::dock::{
    AreaId, Axis, DockArea, DockController, DockFaults, DockNode, DockOp, DockRequest,
    DockSplitter, DockTabStrip, DockTree, DropTarget, DropZone, Layout, PanelHost, PanelId, Side,
    dock_area, panel_frame_id,
};
use forge_ui::testing::Harness;
use forge_ui::widgets::{Build, Checkbox, Label};
use forge_ui::{
    InputEvent, Key, KeyCode, Modifiers, NodeStyle, Point, PointerButton, Rect, Signal, Size, Ui,
    UiConfig, UiError, WidgetId,
};

const NONE: Modifiers = Modifiers::NONE;

/// Counts builds per panel, so a test can prove a re-dock rebuilt nothing.
type Builds = Rc<RefCell<BTreeMap<PanelId, u32>>>;

struct TestHost {
    builds: Builds,
}

impl PanelHost for TestHost {
    fn title(&self, p: &PanelId) -> Option<String> {
        match p.as_str() {
            "forge.hierarchy" => Some("Hierarchy".into()),
            "forge.viewport" => Some("Viewport".into()),
            "forge.inspector" => Some("Inspector".into()),
            "forge.console" => Some("Console".into()),
            "forge.assets" => Some("Assets".into()),
            _ => None,
        }
    }
    fn build(&mut self, b: &mut dyn Build, parent: WidgetId, p: &PanelId) -> Result<bool, UiError> {
        let Some(t) = self.title(p) else {
            return Ok(false);
        };
        *self.builds.borrow_mut().entry(p.clone()).or_default() += 1;
        b.add(parent, "title", NodeStyle::leaf(), Label::new(t.as_str()))?;
        // Panel-local state that a rebuild would lose.
        let on = b.signal(false);
        b.add(
            parent,
            "toggle",
            NodeStyle::leaf(),
            Checkbox::new(on, "Show gizmos"),
        )?;
        Ok(true)
    }
}

fn editor_layout() -> DockNode {
    DockNode::split(
        Axis::Horizontal,
        vec![
            (0.2, DockNode::tabs(&["forge.hierarchy"])),
            (
                0.6,
                DockNode::split(
                    Axis::Vertical,
                    vec![
                        (0.7, DockNode::tabs(&["forge.viewport"])),
                        (0.3, DockNode::tabs(&["forge.console", "forge.assets"])),
                    ],
                ),
            ),
            (
                0.2,
                DockNode::tabs(&["forge.inspector", "forge.unknown_plugin_panel"]),
            ),
        ],
    )
}

struct Dock {
    h: Harness,
    id: WidgetId,
    tree: Signal<DockTree>,
    builds: Builds,
}

fn dock_with(faults: DockFaults) -> Dock {
    let mut h = Harness::new(UiConfig {
        size: Size::new(1000.0, 600.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let builds: Builds = Rc::default();
    let tree = h.ui.rt_mut().signal(DockTree::new(editor_layout()));
    let root = h.ui.root();
    let id = dock_area(
        &mut h.ui,
        root,
        "dock",
        NodeStyle::default().fill(),
        AreaId::Main,
        tree,
        Box::new(TestHost {
            builds: builds.clone(),
        }),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    if let Some(d) = h.ui.widget_mut::<DockArea>(id) {
        d.faults = faults;
    }
    h.settle();
    Dock {
        h,
        id,
        tree,
        builds,
    }
}

fn dock() -> Dock {
    dock_with(DockFaults::default())
}

fn p(s: &str) -> PanelId {
    PanelId::new(s)
}

impl Dock {
    fn area(&self) -> &DockArea {
        self.h
            .ui
            .widget::<DockArea>(self.id)
            .unwrap_or_else(|| panic!("no dock area"))
    }
    fn strip_of(&self, panel: &str) -> WidgetId {
        let a = self.area();
        let gi = a
            .geometry()
            .groups
            .iter()
            .position(|g| g.panels.contains(&p(panel)))
            .unwrap_or_else(|| panic!("{panel} is in no group"));
        a.strips()[gi]
    }
    /// The centre of `panel`'s tab.
    fn tab_center(&self, panel: &str) -> Point {
        let s = self.strip_of(panel);
        let r = self.h.ui.rect(s).unwrap_or_default();
        let strip = self
            .h
            .ui
            .widget::<DockTabStrip>(s)
            .unwrap_or_else(|| panic!("no strip"));
        let i = strip
            .panels()
            .iter()
            .position(|q| *q == p(panel))
            .unwrap_or(0);
        let t = strip.tab_rect(r, i).unwrap_or(r);
        Point::new(t.x + (t.w - 18.0) * 0.5, t.center().y)
    }
    fn frame(&self, panel: &str) -> WidgetId {
        panel_frame_id(self.id, &p(panel))
    }
    fn frame_rect(&self, panel: &str) -> Rect {
        self.h.ui.rect(self.frame(panel)).unwrap_or_default()
    }
    fn tree(&self) -> DockTree {
        self.tree.get(self.h.ui.rt())
    }
    /// Press on `from`, move to `to` in steps, and stop before releasing.
    fn drag_hold(&mut self, from: Point, to: Point) {
        self.h.ui.handle(InputEvent::PointerMoved(from));
        self.h.ui.handle(InputEvent::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
        });
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            self.h.ui.handle(InputEvent::PointerMoved(Point::new(
                from.x + (to.x - from.x) * t,
                from.y + (to.y - from.y) * t,
            )));
        }
        self.h.settle();
    }
    fn release(&mut self, at: Point) {
        self.h.ui.handle(InputEvent::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
        });
        self.h.settle();
    }
}

// ---- layout ------------------------------------------------------------------------------

#[test]
fn groups_strips_and_handles_are_laid_out_from_the_tree() {
    let d = dock();
    let a = d.area();
    assert_eq!(a.geometry().groups.len(), 4);
    assert_eq!(a.strips().len(), 4);
    assert_eq!(a.handles().len(), 3);
    let area = d.h.ui.rect(d.id).unwrap_or_default();
    for g in &a.geometry().groups {
        let (strip, body) = g.rects(area);
        let active = g.active_panel().cloned().unwrap_or_else(|| p("?"));
        let fr = d.frame_rect(active.as_str());
        assert!(
            (fr.x - body.x).abs() < 0.5
                && (fr.y - body.y).abs() < 0.5
                && (fr.w - body.w).abs() < 0.5
                && (fr.h - body.h).abs() < 0.5,
            "{active}: frame {fr:?} != body {body:?}"
        );
        let sid = a.strips()[a.geometry().groups.iter().position(|x| x == g).unwrap_or(0)];
        let sr = d.h.ui.rect(sid).unwrap_or_default();
        assert!(
            (sr.y - strip.y).abs() < 0.5 && (sr.h - strip.h).abs() < 0.5,
            "{sr:?} {strip:?}"
        );
    }
    // The inactive tab's frame exists but is hidden; the active one is shown.
    assert!(d.h.ui.is_hidden(d.frame("forge.assets")));
    assert!(!d.h.ui.is_hidden(d.frame("forge.console")));
    // The hierarchy column is 20 % of the width, less half a handle.
    let hr = d.frame_rect("forge.hierarchy");
    assert!((hr.w - (200.0 - 3.0)).abs() < 0.5, "{hr:?}");
}

#[test]
fn an_unknown_panel_keeps_its_place_and_shows_why() {
    let mut d = dock();
    // Its tab is there (titled by its id) and the layout still holds it.
    assert!(
        d.tree()
            .root
            .panels()
            .contains(&p("forge.unknown_plugin_panel"))
    );
    let pt = d.tab_center("forge.unknown_plugin_panel");
    d.h.click_at(pt);
    let f = d.frame("forge.unknown_plugin_panel");
    assert!(!d.h.ui.is_hidden(f));
    let missing = f.child(&Key::Static("missing"));
    let text =
        d.h.node(missing)
            .and_then(|n| n.label().map(str::to_string))
            .unwrap_or_default();
    assert!(
        text.contains("forge.unknown_plugin_panel") && text.contains("not loaded"),
        "{text}"
    );
}

// ---- drag to dock --------------------------------------------------------------------------

/// Drag the console tab onto the right side of the viewport: a preview shows while it
/// hovers, and on release the console sits right of the viewport — the same widgets,
/// with their state, never rebuilt.
fn check_drag_to_side(d: &mut Dock) -> Result<(), String> {
    let console_frame = d.frame("forge.console");
    let cb = console_frame.child(&Key::Static("toggle"));
    d.h.click(cb); // panel-local state: the checkbox is now on
    d.h.settle();
    let from = d.tab_center("forge.console");
    let vr = d.frame_rect("forge.viewport");
    let to = Point::new(vr.right() - vr.w * 0.1, vr.center().y);
    d.drag_hold(from, to);
    let pending = d.area().pending_drop().map(|h| h.target.clone());
    let want = DropTarget::Group {
        area: AreaId::Main,
        anchor: p("forge.viewport"),
        zone: DropZone::Side(Side::Right),
    };
    if pending.as_ref() != Some(&want) {
        return Err(format!(
            "while hovering: pending drop {pending:?}, want {want:?}"
        ));
    }
    let pv = d.area().preview().ok_or("no preview widget")?;
    if d.h.ui.is_hidden(pv) {
        return Err("no drop preview while hovering".into());
    }
    let pr = d.h.ui.rect(pv).unwrap_or_default();
    if !(pr.x > vr.center().x - 1.0 && (pr.right() - vr.right()).abs() < 1.0) {
        return Err(format!("preview {pr:?} is not the right half of {vr:?}"));
    }
    d.release(to);
    if !d.h.ui.is_hidden(pv) {
        return Err("preview still shown after the drop".into());
    }
    let t = d.tree();
    let path = t
        .root
        .group_path(&p("forge.console"))
        .ok_or("console vanished")?;
    let vpath = t
        .root
        .group_path(&p("forge.viewport"))
        .ok_or("viewport vanished")?;
    // Viewport and console are now siblings of a horizontal split, console on the right.
    let (parent, ci) = path.split_at(path.len() - 1);
    let (vparent, vi) = vpath.split_at(vpath.len() - 1);
    let beside = parent == vparent
        && ci[0] == vi[0] + 1
        && matches!(
            t.root.at(parent),
            Some(DockNode::Split {
                axis: Axis::Horizontal,
                ..
            })
        );
    if !beside {
        return Err(format!(
            "console is not right of the viewport: {:?}",
            t.root
        ));
    }
    // Same widget, same state, one build.
    let builds = d
        .builds
        .borrow()
        .get(&p("forge.console"))
        .copied()
        .unwrap_or(0);
    if !d.h.ui.contains(console_frame) || builds != 1 {
        return Err(format!(
            "the console was rebuilt by the re-dock ({builds} builds; frame kept: {})",
            d.h.ui.contains(console_frame)
        ));
    }
    // The console group's frame is on the right of the viewport's, side by side.
    let (c, v) = (
        d.frame_rect("forge.console"),
        d.frame_rect("forge.viewport"),
    );
    if !(c.x > v.right() && (c.y - v.y).abs() < 0.5) {
        return Err(format!(
            "frames not side by side: console {c:?} viewport {v:?}"
        ));
    }
    let on =
        d.h.node(cb)
            .and_then(|n| n.toggled())
            .is_some_and(|t| t == accesskit::Toggled::True);
    if !on {
        return Err("the console's checkbox state was lost by the re-dock".into());
    }
    Ok(())
}

#[test]
fn dragging_a_tab_onto_a_group_side_splits_it_without_rebuilding_the_panel() {
    let mut d = dock();
    check_drag_to_side(&mut d).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_rebuilding_dock_fails_the_state_check() {
    let mut d = dock_with(DockFaults {
        rebuild_panels_on_change: true,
        ..DockFaults::default()
    });
    let e = check_drag_to_side(&mut d).expect_err("a rebuilding dock kept panel state");
    assert!(e.contains("rebuilt") || e.contains("lost"), "{e}");
}

#[test]
fn positive_control_a_dock_ignoring_drops_fails_the_drag_check() {
    let mut d = dock_with(DockFaults {
        ignore_drops: true,
        ..DockFaults::default()
    });
    let e = check_drag_to_side(&mut d).expect_err("a dock ignoring drops passed");
    assert!(e.contains("not right of the viewport"), "{e}");
}

#[test]
fn dragging_a_tab_onto_another_strip_inserts_it_there() {
    let mut d = dock();
    let from = d.tab_center("forge.hierarchy");
    // Just left of the inspector tab's centre: insert before it.
    let insp = d.tab_center("forge.inspector");
    let to = Point::new(insp.x - 20.0, insp.y);
    d.drag_hold(from, to);
    let pending = d.area().pending_drop().map(|h| h.target.clone());
    assert_eq!(
        pending,
        Some(DropTarget::Group {
            area: AreaId::Main,
            anchor: p("forge.inspector"),
            zone: DropZone::Tab(Some(0)),
        })
    );
    d.release(to);
    match d.tree().root.at(&d
        .tree()
        .root
        .group_path(&p("forge.inspector"))
        .unwrap_or_default())
    {
        Some(DockNode::Tabs { panels, active }) => {
            assert_eq!(panels[0], p("forge.hierarchy"), "{panels:?}");
            assert_eq!(
                panels[*active],
                p("forge.hierarchy"),
                "the dropped tab is shown"
            );
        }
        other => panic!("{other:?}"),
    }
    // The hierarchy's own column collapsed away.
    assert_eq!(d.area().geometry().groups.len(), 3);
}

#[test]
fn dragging_a_tab_along_its_own_strip_reorders_it() {
    let mut d = dock();
    let from = d.tab_center("forge.console");
    let assets = d.tab_center("forge.assets");
    let to = Point::new(assets.x + 30.0, assets.y);
    d.drag_hold(from, to);
    d.release(to);
    match d.tree().root.at(&d
        .tree()
        .root
        .group_path(&p("forge.console"))
        .unwrap_or_default())
    {
        Some(DockNode::Tabs { panels, .. }) => {
            assert_eq!(panels, &vec![p("forge.assets"), p("forge.console")]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn dropping_at_the_window_edge_docks_along_the_whole_side() {
    let mut d = dock();
    let from = d.tab_center("forge.inspector");
    let to = Point::new(500.0, 596.0); // the bottom edge band, over no strip
    d.drag_hold(from, to);
    assert_eq!(
        d.area().pending_drop().map(|h| h.target.clone()),
        Some(DropTarget::AreaEdge {
            area: AreaId::Main,
            side: Side::Bottom
        })
    );
    d.release(to);
    match &d.tree().root {
        DockNode::Split { axis, children, .. } => {
            assert_eq!(*axis, Axis::Vertical);
            assert_eq!(
                children.last().map(DockNode::panels),
                Some(vec![p("forge.inspector")])
            );
        }
        other => panic!("{other:?}"),
    }
    let r = d.frame_rect("forge.inspector");
    assert!((r.w - 1000.0).abs() < 0.5, "along the whole width: {r:?}");
}

#[test]
fn releasing_outside_every_dock_area_tears_the_panel_off() {
    let mut d = dock();
    d.h.ui
        .set_window_origin(Point::new(100.0, 50.0), Some("DISPLAY1".into()));
    let from = d.tab_center("forge.console");
    let to = Point::new(1150.0, 300.0); // right of the window: no dock area there
    d.drag_hold(from, to);
    assert!(d.area().pending_drop().is_none());
    d.release(to);
    let reqs = d.h.take::<DockRequest>();
    let float = reqs.iter().find_map(|r| match &r.op {
        DockOp::Float {
            panel,
            rect,
            monitor,
        } => Some((panel.clone(), *rect, monitor.clone())),
        _ => None,
    });
    let Some((panel, rect, monitor)) = float else {
        panic!("no tear-off request reached the application: {reqs:?}");
    };
    assert_eq!(panel, p("forge.console"));
    assert!(
        (rect.x - (100.0 + 1150.0 - 40.0)).abs() < 1.0
            && (rect.y - (50.0 + 300.0 - 12.0)).abs() < 1.0,
        "the window opens at the drop point on the desktop: {rect:?}"
    );
    assert_eq!(monitor.as_deref(), Some("DISPLAY1"));
    // The local tree is unchanged until the application opens the window.
    assert!(d.tree().root.panels().contains(&p("forge.console")));
}

#[test]
fn escape_cancels_a_drag_and_changes_nothing() {
    let mut d = dock();
    let before = d.tree();
    let from = d.tab_center("forge.console");
    let vr = d.frame_rect("forge.viewport");
    let to = Point::new(vr.x + 10.0, vr.center().y);
    d.drag_hold(from, to);
    assert!(d.area().pending_drop().is_some());
    d.h.press(KeyCode::Escape, NONE);
    let pv = d.area().preview().unwrap_or(WidgetId::ROOT);
    assert!(
        d.h.ui.is_hidden(pv),
        "the preview goes with the cancelled drag"
    );
    d.release(to);
    assert_eq!(d.tree(), before);
    assert!(
        d.h.take::<DockRequest>()
            .iter()
            .all(|r| !matches!(r.op, DockOp::Float { .. }))
    );
}

// ---- splitters, maximise, keyboard ---------------------------------------------------------

#[test]
fn a_splitter_drags_and_steps_from_the_keyboard() {
    let mut d = dock();
    let h0 = d.area().handles()[0];
    let r = d.h.ui.rect(h0).unwrap_or_default();
    let (a, b) = (r.center(), Point::new(r.center().x + 100.0, r.center().y));
    d.h.drag(a, b, 5);
    let hr = d.frame_rect("forge.hierarchy");
    assert!(
        (hr.w - (300.0 - 3.0)).abs() < 1.0,
        "dragged to 30 %: {hr:?}"
    );
    d.h.focus(h0);
    d.h.press(KeyCode::Left, Modifiers::SHIFT);
    let hr = d.frame_rect("forge.hierarchy");
    assert!(
        (hr.w - (200.0 - 3.0)).abs() < 1.5,
        "Shift+Left moves 10 %: {hr:?}"
    );
    let node = d.h.node(h0).unwrap_or_else(|| panic!("no splitter node"));
    assert_eq!(node.role(), accesskit::Role::Splitter);
    assert!(node.numeric_value().is_some_and(|v| (v - 20.0).abs() < 0.5));
    let _ = d.h.ui.widget::<DockSplitter>(h0).map(DockSplitter::handle);
}

#[test]
fn shift_space_maximises_the_focused_panel_and_escape_restores() {
    let mut d = dock();
    let cb = d.frame("forge.viewport").child(&Key::Static("toggle"));
    d.h.focus(cb);
    d.h.press(KeyCode::Space, Modifiers::SHIFT);
    assert_eq!(d.tree().maximised, Some(p("forge.viewport")));
    let r = d.frame_rect("forge.viewport");
    assert!(
        (r.w - 1000.0).abs() < 0.5 && (r.h - (600.0 - 28.0)).abs() < 0.5,
        "{r:?}"
    );
    assert!(d.h.ui.is_hidden(d.frame("forge.hierarchy")));
    assert!(d.area().handles().is_empty());
    assert_eq!(
        d.h.ui.focused(),
        Some(cb),
        "focus stays in the maximised panel"
    );
    d.h.press(KeyCode::Escape, NONE);
    assert_eq!(d.tree().maximised, None);
    assert!(!d.h.ui.is_hidden(d.frame("forge.hierarchy")));
}

#[test]
fn a_tab_strip_switches_reorders_and_closes_from_the_keyboard() {
    let mut d = dock();
    let s = d.strip_of("forge.console");
    d.h.focus(s);
    d.h.press(KeyCode::Right, NONE);
    assert!(!d.h.ui.is_hidden(d.frame("forge.assets")));
    assert!(d.h.ui.is_hidden(d.frame("forge.console")));
    d.h.press(
        KeyCode::Left,
        Modifiers {
            ctrl: true,
            shift: true,
            ..NONE
        },
    );
    match d.tree().root.at(&d
        .tree()
        .root
        .group_path(&p("forge.assets"))
        .unwrap_or_default())
    {
        Some(DockNode::Tabs { panels, .. }) => assert_eq!(panels[0], p("forge.assets")),
        other => panic!("{other:?}"),
    }
    d.h.press(KeyCode::Delete, NONE);
    assert!(
        !d.tree().root.panels().contains(&p("forge.assets")),
        "closed"
    );
    assert!(
        !d.h.ui.contains(d.frame("forge.assets")),
        "its frame is gone"
    );
    let node =
        d.h.node(d.strip_of("forge.console"))
            .unwrap_or_else(|| panic!("no strip"));
    assert_eq!(node.role(), accesskit::Role::TabList);
    let tab =
        d.h.child_node(d.strip_of("forge.console"), 0)
            .unwrap_or_else(|| panic!("no tab node"));
    assert_eq!(tab.role(), accesskit::Role::Tab);
    assert_eq!(tab.label(), Some("Console"));
}

#[test]
fn every_change_is_reported_for_persistence() {
    let mut d = dock();
    let _ = d.h.take::<forge_ui::dock::DockChanged>();
    let s = d.strip_of("forge.console");
    d.h.focus(s);
    d.h.press(KeyCode::Right, NONE);
    let changed = d.h.take::<forge_ui::dock::DockChanged>();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].tree, d.tree());
    // Idle: nothing changes, nothing is reported, nothing redraws.
    let frames = d.h.advance(std::time::Duration::from_secs(2));
    assert_eq!(frames, 0);
    assert!(d.h.take::<forge_ui::dock::DockChanged>().is_empty());
}

// ---- multi-window --------------------------------------------------------------------------

fn host_factory(builds: Builds) -> forge_ui::dock::HostFactory {
    Box::new(move |_| {
        Box::new(TestHost {
            builds: builds.clone(),
        })
    })
}

fn window(size: Size, origin: Point) -> Ui {
    let mut ui = Ui::new(UiConfig {
        size,
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    ui.set_window_origin(origin, Some("DISPLAY1".into()));
    ui
}

fn pump(c: &mut DockController, windows: &mut [(AreaId, &mut Ui)]) {
    for (_, ui) in windows.iter_mut() {
        ui.frame(std::time::Duration::ZERO);
        let acts = ui.take_actions();
        let _ = c.handle_actions(acts);
    }
    for (area, ui) in windows.iter_mut() {
        c.sync_window(ui, *area);
        ui.frame(std::time::Duration::ZERO);
    }
}

#[test]
fn a_torn_off_panel_gets_its_own_window_and_comes_back_when_it_closes() {
    let builds: Builds = Rc::default();
    let mut c = DockController::new(Layout::new(editor_layout()), host_factory(builds));
    let mut main = window(Size::new(1000.0, 600.0), Point::new(0.0, 0.0));
    let root = main.root();
    c.build_main(&mut main, root, "dock", NodeStyle::default().fill())
        .unwrap_or_else(|e| panic!("{e}"));
    pump(&mut c, &mut [(AreaId::Main, &mut main)]);
    // The strip requests a tear-off (as a drag released outside does).
    main.raise(
        root,
        DockRequest {
            area: AreaId::Main,
            op: DockOp::Float {
                panel: p("forge.console"),
                rect: Rect::new(1300.0, 200.0, 500.0, 350.0),
                monitor: Some("DISPLAY2".into()),
            },
        },
    );
    pump(&mut c, &mut [(AreaId::Main, &mut main)]);
    let opens = c.windows_to_open();
    assert_eq!(opens.len(), 1, "{opens:?}");
    let o = &opens[0];
    assert_eq!(o.position, Some(Point::new(1300.0, 200.0)));
    assert!(c.windows_to_open().is_empty(), "requested once");
    // The runner opens it and the app builds its dock area.
    let mut fl = window(o.size, o.position.unwrap_or_default());
    c.build_floating(&mut fl, o.key)
        .unwrap_or_else(|e| panic!("{e}"));
    let fa = AreaId::Floating(o.key);
    pump(&mut c, &mut [(AreaId::Main, &mut main), (fa, &mut fl)]);
    let fdock = c.dock_of(fa).unwrap_or(WidgetId::ROOT);
    assert!(fl.contains(panel_frame_id(fdock, &p("forge.console"))));
    let mdock = c.dock_of(AreaId::Main).unwrap_or(WidgetId::ROOT);
    assert!(
        !main.contains(panel_frame_id(mdock, &p("forge.console"))),
        "moved out of main"
    );
    // The user closes the floating window: its panel returns to the main window.
    c.window_closed(o.key);
    pump(&mut c, &mut [(AreaId::Main, &mut main)]);
    assert!(main.contains(panel_frame_id(mdock, &p("forge.console"))));
    assert!(c.layout().floating.is_empty());
    assert!(c.take_dirty(), "the layout changed and must be saved");
}

#[test]
fn a_panel_dropped_from_a_floating_window_onto_the_main_window_docks_there() {
    let builds: Builds = Rc::default();
    let mut layout = Layout::new(editor_layout());
    let id = layout
        .float(
            &p("forge.assets"),
            Rect::new(1200.0, 100.0, 400.0, 300.0),
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let mut c = DockController::new(layout, host_factory(builds));
    let mut main = window(Size::new(1000.0, 600.0), Point::new(0.0, 0.0));
    let root = main.root();
    c.build_main(&mut main, root, "dock", NodeStyle::default().fill())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut fl = window(Size::new(400.0, 300.0), Point::new(1200.0, 100.0));
    c.build_floating(&mut fl, id)
        .unwrap_or_else(|e| panic!("{e}"));
    let fa = AreaId::Floating(id);
    pump(&mut c, &mut [(AreaId::Main, &mut main), (fa, &mut fl)]);
    // Released over the left edge of the main window (desktop coordinates).
    fl.raise(
        fl.root(),
        DockRequest {
            area: fa,
            op: DockOp::Float {
                panel: p("forge.assets"),
                rect: Rect::new(10.0 - 40.0, 300.0 - 12.0, 400.0, 300.0),
                monitor: None,
            },
        },
    );
    pump(&mut c, &mut [(AreaId::Main, &mut main), (fa, &mut fl)]);
    assert_eq!(
        c.layout().find(&p("forge.assets")).map(|f| f.0),
        Some(AreaId::Main)
    );
    let mdock = c.dock_of(AreaId::Main).unwrap_or(WidgetId::ROOT);
    assert!(main.contains(panel_frame_id(mdock, &p("forge.assets"))));
    assert!(
        c.layout().floating.is_empty(),
        "the emptied window is dropped"
    );
    // The emptied floating window is asked to close.
    let closes = fl
        .take_actions()
        .iter()
        .filter(|a| a.get::<forge_ui::ui::CloseWindow>().is_some())
        .count();
    assert_eq!(closes, 1);
}

#[test]
fn layouts_round_trip_through_ron_with_floating_windows() {
    let mut l = Layout::new(editor_layout());
    l.float(
        &p("forge.console"),
        Rect::new(1920.0, 40.0, 640.0, 480.0),
        Some("DISPLAY2".into()),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    l.toggle_maximise(&p("forge.viewport"));
    let text = l.to_ron().unwrap_or_else(|e| panic!("{e}"));
    let back = Layout::from_ron(&text).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back, l);
    assert!(
        text.contains("forge.unknown_plugin_panel"),
        "unknown ids are written"
    );
}

// ---- cost (D-5) ----------------------------------------------------------------------------

/// What docking costs per interaction frame: a splitter drag restyles the dock's children
/// in the layout engine and re-records only what moved; switching a tab re-records the
/// strip and the two frames involved; nothing is rebuilt and idle stays at zero frames.
/// Milliseconds are printed (the reference machine records them, W5: no tolerance here);
/// the counters are asserted.
#[test]
fn dock_interaction_frames_are_cheap() {
    let mut d = dock();
    let h0 = d.area().handles()[0];
    let r = d.h.ui.rect(h0).unwrap_or_default();
    let start = r.center();
    d.h.ui.handle(InputEvent::PointerMoved(start));
    d.h.ui.handle(InputEvent::PointerButton {
        pos: start,
        button: PointerButton::Primary,
        pressed: true,
    });
    d.h.step();
    let t0 = std::time::Instant::now();
    let mut draws = 0u32;
    for i in 1..=120 {
        d.h.ui.handle(InputEvent::PointerMoved(Point::new(
            start.x + (i % 60) as f32 * 2.0,
            start.y,
        )));
        if let Some(s) = d.h.step() {
            draws = draws.max(s.draw_calls);
        }
    }
    let per_frame = t0.elapsed().as_secs_f64() * 1000.0 / 120.0;
    d.h.ui.handle(InputEvent::PointerButton {
        pos: start,
        button: PointerButton::Primary,
        pressed: false,
    });
    d.h.settle();
    let builds: u32 = d.builds.borrow().values().sum();
    assert_eq!(builds, 5, "a splitter drag rebuilt panels");
    // A tab switch re-records a handful of slices, not the dock.
    let s = d.strip_of("forge.console");
    d.h.focus(s);
    d.h.key(KeyCode::Right, NONE);
    d.h.step();
    let slices = d.h.ui.stats().last_slices;
    assert!(slices <= 12, "a tab switch re-recorded {slices} slices");
    let idle = d.h.advance(std::time::Duration::from_secs(5));
    assert_eq!(idle, 0, "an idle dock draws nothing");
    println!(
        "dock: splitter-drag frame (frame + batch + record-render) {per_frame:.3} ms avg over 120 \
         frames, max {draws} draw calls; tab switch re-recorded {slices} slices; idle 0 frames"
    );
}
