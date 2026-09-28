//! Multi-window docking: one [`Layout`] across the main window and floating OS windows,
//! on any number of monitors (Ch.21 §21.14, §21.17).
//!
//! The [`DockController`] owns the layout. Each OS window has its own `Ui` (its own signal
//! runtime), so each dock area edits its own `Signal<DockTree>`; the controller folds every
//! area's changes into the layout ([`DockChanged`]), handles what one window cannot do
//! alone — tearing a panel off into a new window, dropping a panel from one window into
//! another, closing a floating window — and pushes the result back into every affected
//! window ([`DockController::sync_window`], called by the runner's `update_window` hook).
//!
//! It is runner-agnostic: the winit runner calls it, and the tests drive it with plain
//! `Ui`s standing in for OS windows.

use std::collections::BTreeMap;

use super::area::{DockChanged, DockOp, DockRequest, DockTree, PanelHost, apply_local, dock_area};
use super::geometry::{MonitorInfo, drop_at, geometry, place_window};
use super::model::{AreaId, Layout, PanelId};
use crate::UiError;
use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::layout::NodeStyle;
use crate::state::Signal;
use crate::ui::{ActionEnvelope, CloseWindow, OpenWindow, Ui};

/// Makes the panel host for a newly built dock area.
pub type HostFactory = Box<dyn FnMut(AreaId) -> Box<dyn PanelHost>>;

struct Slot {
    tree: Signal<DockTree>,
    dock: WidgetId,
    /// The layout generation this window last showed.
    synced: u64,
    /// The dock area's rect inside its window, and the window's client origin on the
    /// desktop (both logical px): where the area is on the virtual screen.
    dock_rect: Rect,
    origin: Point,
}

/// Owns the multi-window layout (see the module docs).
pub struct DockController {
    layout: Layout,
    generation: u64,
    slots: BTreeMap<AreaId, Slot>,
    requested: BTreeMap<u64, ()>,
    monitors: Vec<MonitorInfo>,
    hosts: HostFactory,
    dirty: bool,
}

impl DockController {
    pub fn new(layout: Layout, hosts: HostFactory) -> Self {
        Self {
            layout,
            generation: 1,
            slots: BTreeMap::new(),
            requested: BTreeMap::new(),
            monitors: Vec::new(),
            hosts,
            dirty: false,
        }
    }

    /// The whole layout (what autosave writes).
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Replace the layout (a preset or a saved user layout was chosen). Every window
    /// re-syncs; floating windows that are gone close, new ones are requested.
    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        self.bump();
    }

    /// Changes since the last call (for a debounced autosave).
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn bump(&mut self) {
        self.generation += 1;
        self.dirty = true;
    }

    /// The monitors the platform reports (placement of floating windows).
    pub fn set_monitors(&mut self, monitors: Vec<MonitorInfo>) {
        self.monitors = monitors;
    }

    fn tree_for(&self, area: AreaId) -> Option<DockTree> {
        let root = self.layout.root(area)?.clone();
        let maximised = self
            .layout
            .maximised
            .clone()
            .filter(|m| root.group_path(m).is_some());
        Some(DockTree { root, maximised })
    }

    fn build(
        &mut self,
        ui: &mut Ui,
        parent: WidgetId,
        key: Key,
        style: NodeStyle,
        area: AreaId,
    ) -> Result<WidgetId, UiError> {
        let tree = self
            .tree_for(area)
            .ok_or_else(|| UiError::Dock(format!("no dock area {area:?} in the layout")))?;
        let sig = ui.rt_mut().signal(tree);
        let host = (self.hosts)(area);
        let dock = dock_area(ui, parent, key, style, area, sig, host)?;
        self.slots.insert(
            area,
            Slot {
                tree: sig,
                dock,
                synced: self.generation,
                dock_rect: Rect::ZERO,
                origin: ui.window_origin(),
            },
        );
        Ok(dock)
    }

    /// Build the main window's dock area under `parent`.
    pub fn build_main(
        &mut self,
        ui: &mut Ui,
        parent: WidgetId,
        key: impl Into<Key>,
        style: NodeStyle,
    ) -> Result<WidgetId, UiError> {
        self.build(ui, parent, key.into(), style, AreaId::Main)
    }

    /// Build floating window `key`'s dock area (it fills the window).
    pub fn build_floating(&mut self, ui: &mut Ui, key: u64) -> Result<WidgetId, UiError> {
        self.requested.remove(&key);
        let root = ui.root();
        self.build(
            ui,
            root,
            Key::Static("dock"),
            NodeStyle::default().fill(),
            AreaId::Floating(key),
        )
    }

    /// Floating windows the layout has but no OS window shows yet, placed on the monitors
    /// that exist now. Each is returned once; the runner opens them and calls
    /// [`DockController::build_floating`].
    pub fn windows_to_open(&mut self) -> Vec<OpenWindow> {
        let mut out = Vec::new();
        for w in &self.layout.floating {
            let area = AreaId::Floating(w.id);
            if self.slots.contains_key(&area) || self.requested.contains_key(&w.id) {
                continue;
            }
            let (rect, monitor) = place_window(w.rect, w.monitor.as_deref(), &self.monitors);
            let title = w
                .root
                .panels()
                .first()
                .map_or_else(|| crate::tr!("Panel").to_string(), PanelId::to_string);
            out.push(OpenWindow {
                key: w.id,
                title,
                size: Size::new(rect.w, rect.h),
                position: Some(Point::new(rect.x, rect.y)),
                monitor,
            });
        }
        for o in &out {
            self.requested.insert(o.key, ());
        }
        out
    }

    /// Take the dock actions out of `actions` (from any window) and act on them; the
    /// others are returned for the application.
    pub fn handle_actions(&mut self, actions: Vec<ActionEnvelope>) -> Vec<ActionEnvelope> {
        let mut rest = Vec::new();
        for a in actions {
            if let Some(c) = a.get::<DockChanged>() {
                self.on_changed(c.clone());
            } else if let Some(r) = a.get::<DockRequest>() {
                self.on_request(r.clone());
            } else {
                rest.push(a);
            }
        }
        rest
    }

    fn on_changed(&mut self, c: DockChanged) {
        let before = self.layout.clone();
        if let Some(m) = &c.tree.maximised {
            self.layout.maximised = Some(m.clone());
        } else if self
            .layout
            .maximised
            .as_ref()
            .is_some_and(|m| c.tree.root.group_path(m).is_some() || !self.layout.contains(m))
        {
            self.layout.maximised = None;
        }
        self.layout.set_root(c.area, c.tree.root);
        if self.layout != before {
            // The window that changed already shows this: only the others re-sync.
            self.bump();
            if let Some(s) = self.slots.get_mut(&c.area) {
                s.synced = self.generation;
            }
        }
    }

    fn on_request(&mut self, r: DockRequest) {
        match r.op {
            DockOp::Float {
                panel,
                rect,
                monitor,
            } => {
                // Released over another window's dock area: dock it there instead.
                let at = Point::new(rect.x + 40.0, rect.y + 12.0);
                if self.drop_at_screen(&panel, at) {
                    return;
                }
                if self.layout.float(&panel, rect, monitor).is_ok() {
                    self.bump();
                }
            }
            op => {
                if let Some(tree) = self.tree_for(r.area)
                    && let Some(next) = apply_local(&tree, &op)
                {
                    self.on_changed(DockChanged {
                        area: r.area,
                        tree: next,
                    });
                    if let Some(s) = self.slots.get_mut(&r.area) {
                        // Not shown there yet: make it re-sync.
                        s.synced = 0;
                    }
                }
            }
        }
    }

    /// Drop `panel` at desktop point `p`: onto the dock area of whichever window is there
    /// (not the panel's own single-panel floating window). Returns whether it docked.
    pub fn drop_at_screen(&mut self, panel: &PanelId, p: Point) -> bool {
        let own = self.layout.find(panel).map(|f| f.0);
        let hit = self.slots.iter().find_map(|(area, s)| {
            let screen = s.dock_rect.translate(s.origin.x, s.origin.y);
            if !screen.contains(p) {
                return None;
            }
            let tree = self.tree_for(*area)?;
            if Some(*area) == own && tree.root.panels().len() == 1 {
                return None; // its own lone window: that is just moving the window
            }
            let g = geometry(&tree.root, tree.maximised.as_ref());
            drop_at(*area, &g, screen, p, panel, &|_, _| None).map(|h| h.target)
        });
        match hit {
            Some(target) => {
                let ok = self.layout.move_panel(panel, &target).is_ok();
                if ok {
                    self.bump();
                }
                ok
            }
            None => false,
        }
    }

    /// Bring window `area`'s dock in line with the layout (the runner calls this for every
    /// window after actions). A floating window whose tree emptied is asked to close.
    pub fn sync_window(&mut self, ui: &mut Ui, area: AreaId) {
        let Some(slot) = self.slots.get(&area) else {
            return;
        };
        let (dock, sig, synced) = (slot.dock, slot.tree, slot.synced);
        let rect = ui.rect(dock).unwrap_or_default();
        let origin = ui.window_origin();
        if let Some(s) = self.slots.get_mut(&area) {
            s.dock_rect = rect;
            s.origin = origin;
        }
        if synced >= self.generation {
            return;
        }
        match self.tree_for(area) {
            Some(t) => {
                sig.set(ui.rt_mut(), t);
                if let Some(s) = self.slots.get_mut(&area) {
                    s.synced = self.generation;
                }
            }
            None => {
                self.slots.remove(&area);
                let root = ui.root();
                ui.raise(root, CloseWindow);
            }
        }
    }

    /// The platform moved or resized window `area` (desktop logical px) onto `monitor`.
    pub fn window_moved(&mut self, area: AreaId, rect: Rect, monitor: Option<String>) {
        if let Some(s) = self.slots.get_mut(&area) {
            s.origin = Point::new(rect.x, rect.y);
        }
        if let AreaId::Floating(id) = area
            && self
                .layout
                .floating_window(id)
                .is_some_and(|w| w.rect != rect || w.monitor != monitor)
        {
            self.layout.set_floating_rect(id, rect, monitor);
            self.dirty = true;
        }
    }

    /// The user closed floating window `key` with the OS close button: its panels go back
    /// into the main window (closing a window never loses panels).
    pub fn window_closed(&mut self, key: u64) {
        self.slots.remove(&AreaId::Floating(key));
        self.requested.remove(&key);
        if self.layout.dock_back(key) {
            self.bump();
        }
    }

    /// The dock area widget of `area`, if built.
    pub fn dock_of(&self, area: AreaId) -> Option<WidgetId> {
        self.slots.get(&area).map(|s| s.dock)
    }
}
