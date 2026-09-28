//! The docking model: layouts as data (Ch.21 §21.17).
//!
//! A [`Layout`] is a tree of splits and tab groups for the main window plus any floating
//! windows, and the maximised panel. It is plain data: it serialises to RON
//! ([`Layout::to_ron`] / [`Layout::from_ron`]), every edit is a pure operation on it
//! ([`Layout::move_panel`], [`Layout::float`], ...), and after every edit it is
//! **normalised** — no empty groups, no one-child splits, no nested same-axis splits, ratios
//! finite and summing to one — so a layout file a user edited by hand, or one written by an
//! older editor, always loads into a valid tree.
//!
//! **Unknown panel ids are kept.** A [`PanelId`] is just the panel's extension-point id
//! (`"forge.hierarchy"`); nothing here checks it against a registry, so a layout naming a
//! panel whose plugin is disabled round-trips unchanged and shows a placeholder until the
//! plugin returns (`test_layout_unknown_panel_kept`).

use serde::{Deserialize, Serialize};

use crate::UiError;
use crate::geom::Rect;

/// The layout file format version this build writes and the newest it reads.
pub const LAYOUT_VERSION: u32 = 1;

/// The smallest share of a split a child keeps (so a collapsed panel stays grabbable).
pub const MIN_RATIO: f32 = 0.02;

/// A panel's stable extension-point id (`"forge.hierarchy"`, §21.21).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PanelId(pub String);

impl PanelId {
    pub fn new(id: &str) -> Self {
        Self(id.to_string())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PanelId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl std::fmt::Display for PanelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The direction a split lays its children out in.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    /// Side by side, left to right.
    Horizontal,
    /// Stacked, top to bottom.
    Vertical,
}

/// A node of a dock tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DockNode {
    /// Children laid out along `axis`, child `i` taking `ratios[i]` of the space.
    Split {
        axis: Axis,
        ratios: Vec<f32>,
        children: Vec<DockNode>,
    },
    /// A tab group: one panel shown, the others one click away.
    Tabs { panels: Vec<PanelId>, active: usize },
}

impl DockNode {
    /// A tab group of `panels` with the first active.
    pub fn tabs(panels: &[&str]) -> Self {
        DockNode::Tabs {
            panels: panels.iter().map(|p| PanelId::new(p)).collect(),
            active: 0,
        }
    }

    /// A split with the given ratios (normalised on use).
    pub fn split(axis: Axis, parts: Vec<(f32, DockNode)>) -> Self {
        let (ratios, children) = parts.into_iter().unzip();
        DockNode::Split {
            axis,
            ratios,
            children,
        }
    }

    /// An empty tab group (an empty main area).
    pub fn empty() -> Self {
        DockNode::Tabs {
            panels: Vec::new(),
            active: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            DockNode::Tabs { panels, .. } => panels.is_empty(),
            DockNode::Split { children, .. } => children.iter().all(DockNode::is_empty),
        }
    }

    /// Every panel in paint order (depth first).
    pub fn panels(&self) -> Vec<PanelId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PanelId>) {
        match self {
            DockNode::Tabs { panels, .. } => out.extend(panels.iter().cloned()),
            DockNode::Split { children, .. } => {
                for c in children {
                    c.collect(out);
                }
            }
        }
    }

    /// The path (child indices from this node) of the tab group holding `panel`.
    pub fn group_path(&self, panel: &PanelId) -> Option<Vec<usize>> {
        match self {
            DockNode::Tabs { panels, .. } => panels.contains(panel).then(Vec::new),
            DockNode::Split { children, .. } => children.iter().enumerate().find_map(|(i, c)| {
                c.group_path(panel).map(|mut p| {
                    p.insert(0, i);
                    p
                })
            }),
        }
    }

    /// The node at `path`.
    pub fn at(&self, path: &[usize]) -> Option<&DockNode> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                DockNode::Split { children, .. } => children.get(*i)?.at(rest),
                DockNode::Tabs { .. } => None,
            },
        }
    }

    /// The node at `path`, mutably.
    pub fn at_mut(&mut self, path: &[usize]) -> Option<&mut DockNode> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                DockNode::Split { children, .. } => children.get_mut(*i)?.at_mut(rest),
                DockNode::Tabs { .. } => None,
            },
        }
    }

    /// Remove `panel` wherever it is. Returns whether it was found. Leaves empty groups
    /// behind: call [`DockNode::normalized`] afterwards.
    fn remove_panel(&mut self, panel: &PanelId) -> bool {
        match self {
            DockNode::Tabs { panels, active } => match panels.iter().position(|p| p == panel) {
                Some(i) => {
                    panels.remove(i);
                    if *active > i || (*active == i && *active >= panels.len()) {
                        *active = active.saturating_sub(1);
                    }
                    true
                }
                None => false,
            },
            DockNode::Split { children, .. } => children.iter_mut().any(|c| c.remove_panel(panel)),
        }
    }

    /// The normalised form of this node, or `None` if it holds no panel.
    pub fn normalized(self) -> Option<DockNode> {
        match self {
            DockNode::Tabs { panels, active } => {
                if panels.is_empty() {
                    None
                } else {
                    let active = active.min(panels.len() - 1);
                    Some(DockNode::Tabs { panels, active })
                }
            }
            DockNode::Split {
                axis,
                ratios,
                children,
            } => {
                let n = children.len();
                let fair = if n == 0 { 1.0 } else { 1.0 / n as f32 };
                let ratios: Vec<f32> = if ratios.len() == n {
                    ratios
                        .into_iter()
                        .map(|r| if r.is_finite() && r > 0.0 { r } else { fair })
                        .collect()
                } else {
                    vec![fair; n]
                };
                let mut kept: Vec<(f32, DockNode)> = Vec::with_capacity(n);
                for (r, c) in ratios.into_iter().zip(children) {
                    let Some(c) = c.normalized() else { continue };
                    match c {
                        // A child split along the same axis melts into this one, its
                        // children sharing its ratio.
                        DockNode::Split {
                            axis: a,
                            ratios: rs,
                            children: cs,
                        } if a == axis => {
                            let sum: f32 = rs.iter().sum();
                            for (cr, cc) in rs.into_iter().zip(cs) {
                                kept.push((r * cr / sum.max(f32::EPSILON), cc));
                            }
                        }
                        other => kept.push((r, other)),
                    }
                }
                match kept.len() {
                    0 => None,
                    1 => kept.pop().map(|(_, c)| c),
                    _ => {
                        let (ratios, children): (Vec<f32>, Vec<DockNode>) =
                            kept.into_iter().unzip();
                        Some(DockNode::Split {
                            axis,
                            ratios: normalize_ratios(ratios),
                            children,
                        })
                    }
                }
            }
        }
    }
}

/// Ratios that are finite, at least [`MIN_RATIO`], and sum to one.
pub fn normalize_ratios(mut ratios: Vec<f32>) -> Vec<f32> {
    if ratios.is_empty() {
        return ratios;
    }
    let n = ratios.len() as f32;
    for r in &mut ratios {
        if !r.is_finite() || *r <= 0.0 {
            *r = 1.0 / n;
        }
    }
    let min = MIN_RATIO.min(1.0 / n);
    let sum: f32 = ratios.iter().sum();
    // Already normal (within float noise): leave the bits alone, so normalising is
    // idempotent and a saved layout reads back bit for bit.
    if (sum - 1.0).abs() <= 1e-5 && ratios.iter().all(|r| *r >= min - 1e-7) {
        return ratios;
    }
    for r in &mut ratios {
        *r /= sum;
    }
    // Lift anything below the floor, taking the difference from the largest shares.
    let deficit: f32 = ratios.iter().map(|r| (min - r).max(0.0)).sum();
    if deficit > 0.0 {
        let spare: f32 = ratios.iter().map(|r| (r - min).max(0.0)).sum();
        for r in &mut ratios {
            if *r < min {
                *r = min;
            } else if spare > 0.0 {
                *r -= deficit * (*r - min) / spare;
            }
        }
    }
    ratios
}

/// A floating dock window: its own OS window with its own dock tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloatingWindow {
    /// Stable within the layout (the runner's window key).
    pub id: u64,
    /// Where it is on the desktop (logical px of the virtual screen).
    pub rect: Rect,
    /// The monitor it was on (a hint: if that monitor is gone it opens on another one).
    #[serde(default)]
    pub monitor: Option<String>,
    pub root: DockNode,
}

/// Which dock tree: the main window's or a floating window's.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AreaId {
    Main,
    Floating(u64),
}

/// A side of a group or of a whole dock area.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn axis(self) -> Axis {
        match self {
            Side::Left | Side::Right => Axis::Horizontal,
            Side::Top | Side::Bottom => Axis::Vertical,
        }
    }
    fn first(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
}

/// Where in a tab group a panel lands.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DropZone {
    /// Into the group as a tab, at this index (`None`: last).
    Tab(Option<usize>),
    /// Beside the group: the group splits and the panel takes this side.
    Side(Side),
}

/// Where a moved panel lands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DropTarget {
    /// Relative to the tab group holding `anchor`.
    Group {
        area: AreaId,
        anchor: PanelId,
        zone: DropZone,
    },
    /// Along a whole area's edge (it takes a quarter of the area).
    AreaEdge { area: AreaId, side: Side },
    /// Into an area that holds no panel.
    EmptyArea(AreaId),
}

impl DropTarget {
    /// The same target in another area (a single-area tree is edited as `Main`).
    pub fn in_area(self, a: AreaId) -> Self {
        match self {
            DropTarget::Group { anchor, zone, .. } => DropTarget::Group {
                area: a,
                anchor,
                zone,
            },
            DropTarget::AreaEdge { side, .. } => DropTarget::AreaEdge { area: a, side },
            DropTarget::EmptyArea(_) => DropTarget::EmptyArea(a),
        }
    }

    pub fn area(&self) -> AreaId {
        match self {
            DropTarget::Group { area, .. }
            | DropTarget::AreaEdge { area, .. }
            | DropTarget::EmptyArea(area) => *area,
        }
    }
}

/// A complete layout: the main window's tree, the floating windows, the maximised panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub version: u32,
    pub main: DockNode,
    #[serde(default)]
    pub floating: Vec<FloatingWindow>,
    #[serde(default)]
    pub maximised: Option<PanelId>,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new(DockNode::empty())
    }
}

fn dock_err(msg: impl Into<String>) -> UiError {
    UiError::Dock(msg.into())
}

impl Layout {
    /// A layout with `main` as the main window's tree (normalised).
    pub fn new(main: DockNode) -> Self {
        let mut l = Layout {
            version: LAYOUT_VERSION,
            main,
            floating: Vec::new(),
            maximised: None,
        };
        l.normalize();
        l
    }

    /// RON text (pretty, stable field order): what presets and user layouts store.
    pub fn to_ron(&self) -> Result<String, UiError> {
        let cfg = ron::ser::PrettyConfig::new()
            .depth_limit(64)
            .indentor("    ".to_string());
        ron::ser::to_string_pretty(self, cfg).map_err(|e| dock_err(format!("serialise: {e}")))
    }

    /// Parse a layout file. A newer format version is refused (UI-0008) rather than read
    /// wrongly; the tree is normalised, so hand-edited files load into a valid layout.
    pub fn from_ron(text: &str) -> Result<Layout, UiError> {
        let mut l: Layout =
            ron::from_str(text).map_err(|e| dock_err(format!("layout file: {e}")))?;
        if l.version == 0 || l.version > LAYOUT_VERSION {
            return Err(dock_err(format!(
                "layout file version {} is not readable by this editor (it reads 1..={LAYOUT_VERSION})",
                l.version
            )));
        }
        l.version = LAYOUT_VERSION;
        l.normalize();
        Ok(l)
    }

    /// Bring the layout to its normal form (see the module docs). Idempotent.
    pub fn normalize(&mut self) {
        // A panel appears once: later duplicates (a hand-edited file) are dropped.
        let mut seen = std::collections::BTreeSet::new();
        dedupe(&mut self.main, &mut seen);
        for w in &mut self.floating {
            dedupe(&mut w.root, &mut seen);
        }
        let main = std::mem::take(&mut self.main);
        self.main = main.normalized().unwrap_or_else(DockNode::empty);
        let floating = std::mem::take(&mut self.floating);
        self.floating = floating
            .into_iter()
            .filter_map(|mut w| {
                let root = std::mem::take(&mut w.root);
                w.root = root.normalized()?;
                if !(w.rect.w.is_finite() && w.rect.w >= 120.0) {
                    w.rect.w = 480.0;
                }
                if !(w.rect.h.is_finite() && w.rect.h >= 80.0) {
                    w.rect.h = 360.0;
                }
                if !w.rect.x.is_finite() {
                    w.rect.x = 0.0;
                }
                if !w.rect.y.is_finite() {
                    w.rect.y = 0.0;
                }
                Some(w)
            })
            .collect();
        let mut ids = std::collections::BTreeSet::new();
        let mut next = self.floating.iter().map(|w| w.id).max().unwrap_or(0) + 1;
        for w in &mut self.floating {
            if w.id == 0 || !ids.insert(w.id) {
                w.id = next;
                ids.insert(next);
                next += 1;
            }
        }
        if let Some(m) = &self.maximised
            && self.find(m).is_none()
        {
            self.maximised = None;
        }
    }

    /// Every panel in the layout.
    pub fn panels(&self) -> Vec<PanelId> {
        let mut out = self.main.panels();
        for w in &self.floating {
            out.extend(w.root.panels());
        }
        out
    }

    pub fn contains(&self, panel: &PanelId) -> bool {
        self.find(panel).is_some()
    }

    /// Where `panel` is: its area and the path of its tab group.
    pub fn find(&self, panel: &PanelId) -> Option<(AreaId, Vec<usize>)> {
        if let Some(p) = self.main.group_path(panel) {
            return Some((AreaId::Main, p));
        }
        self.floating.iter().find_map(|w| {
            w.root
                .group_path(panel)
                .map(|p| (AreaId::Floating(w.id), p))
        })
    }

    /// The tree of `area`.
    pub fn root(&self, area: AreaId) -> Option<&DockNode> {
        match area {
            AreaId::Main => Some(&self.main),
            AreaId::Floating(id) => self.floating.iter().find(|w| w.id == id).map(|w| &w.root),
        }
    }

    fn root_mut(&mut self, area: AreaId) -> Option<&mut DockNode> {
        match area {
            AreaId::Main => Some(&mut self.main),
            AreaId::Floating(id) => self
                .floating
                .iter_mut()
                .find(|w| w.id == id)
                .map(|w| &mut w.root),
        }
    }

    /// The floating window `id`.
    pub fn floating_window(&self, id: u64) -> Option<&FloatingWindow> {
        self.floating.iter().find(|w| w.id == id)
    }

    /// Put a panel that is not in the layout at `target`.
    pub fn insert(&mut self, panel: PanelId, target: &DropTarget) -> Result<(), UiError> {
        if self.contains(&panel) {
            return Err(dock_err(format!("{panel} is already in the layout")));
        }
        let area = target.area();
        let root = self
            .root_mut(area)
            .ok_or_else(|| dock_err(format!("no dock area {area:?}")))?;
        match target {
            DropTarget::EmptyArea(_) => {
                if !root.is_empty() {
                    return Err(dock_err(format!("area {area:?} is not empty")));
                }
                *root = DockNode::Tabs {
                    panels: vec![panel],
                    active: 0,
                };
            }
            DropTarget::AreaEdge { side, .. } => {
                if root.is_empty() {
                    *root = DockNode::Tabs {
                        panels: vec![panel],
                        active: 0,
                    };
                } else {
                    let old = std::mem::take(root);
                    let new = DockNode::Tabs {
                        panels: vec![panel],
                        active: 0,
                    };
                    let (ratios, children) = if side.first() {
                        (vec![0.25, 0.75], vec![new, old])
                    } else {
                        (vec![0.75, 0.25], vec![old, new])
                    };
                    *root = DockNode::Split {
                        axis: side.axis(),
                        ratios,
                        children,
                    };
                }
            }
            DropTarget::Group { anchor, zone, .. } => {
                let path = root
                    .group_path(anchor)
                    .ok_or_else(|| dock_err(format!("{anchor} is not in area {area:?}")))?;
                let group = root
                    .at_mut(&path)
                    .ok_or_else(|| dock_err("dock path went stale"))?;
                match zone {
                    DropZone::Tab(at) => {
                        if let DockNode::Tabs { panels, active } = group {
                            let i = at.unwrap_or(panels.len()).min(panels.len());
                            panels.insert(i, panel);
                            *active = i;
                        }
                    }
                    DropZone::Side(side) => {
                        let old = std::mem::take(group);
                        let new = DockNode::Tabs {
                            panels: vec![panel],
                            active: 0,
                        };
                        let children = if side.first() {
                            vec![new, old]
                        } else {
                            vec![old, new]
                        };
                        *group = DockNode::Split {
                            axis: side.axis(),
                            ratios: vec![0.5, 0.5],
                            children,
                        };
                    }
                }
            }
        }
        self.normalize();
        Ok(())
    }

    /// Move `panel` to `target`. On an invalid target nothing changes. Dropping a panel
    /// onto its own single-panel group is a no-op.
    pub fn move_panel(&mut self, panel: &PanelId, target: &DropTarget) -> Result<(), UiError> {
        if !self.contains(panel) {
            return Err(dock_err(format!("{panel} is not in the layout")));
        }
        if let DropTarget::Group { anchor, zone, area } = target
            && anchor == panel
        {
            // Onto its own group: a tab move is a no-op; a side split needs a sibling.
            let alone = self
                .root(*area)
                .and_then(|r| r.at(&r.group_path(panel)?))
                .is_some_and(|g| matches!(g, DockNode::Tabs { panels, .. } if panels.len() == 1));
            if alone {
                return Ok(());
            }
            if let DropZone::Tab(at) = zone {
                // Along its own strip: a reorder. `at` counts the panel itself, so a later
                // slot shifts down by one once it is lifted out.
                let (a, path) = self.find(panel).ok_or_else(|| dock_err("panel vanished"))?;
                let cur = match self.root(a).and_then(|r| r.at(&path)) {
                    Some(DockNode::Tabs { panels, .. }) => {
                        panels.iter().position(|p| p == panel).unwrap_or(0)
                    }
                    _ => 0,
                };
                let to = match at {
                    Some(i) if *i > cur => i - 1,
                    Some(i) => *i,
                    None => usize::MAX,
                };
                self.reorder(panel, to);
                return Ok(());
            }
            // Split its own group: re-anchor on a sibling of the same group.
            let sibling = self
                .root(*area)
                .and_then(|r| r.at(&r.group_path(panel)?))
                .and_then(|g| match g {
                    DockNode::Tabs { panels, .. } => panels.iter().find(|p| *p != panel).cloned(),
                    DockNode::Split { .. } => None,
                })
                .ok_or_else(|| dock_err("no sibling to split against"))?;
            let t = DropTarget::Group {
                area: *area,
                anchor: sibling,
                zone: *zone,
            };
            return self.move_panel(panel, &t);
        }
        let before = self.clone();
        let was_max = self.maximised.as_ref() == Some(panel);
        self.remove_quiet(panel);
        match self.insert(panel.clone(), target) {
            Ok(()) => {
                if was_max {
                    self.maximised = Some(panel.clone());
                }
                Ok(())
            }
            Err(e) => {
                *self = before;
                Err(e)
            }
        }
    }

    fn remove_quiet(&mut self, panel: &PanelId) -> bool {
        let mut found = self.main.remove_panel(panel);
        for w in &mut self.floating {
            found |= w.root.remove_panel(panel);
        }
        self.normalize();
        found
    }

    /// Close a panel (it can be reopened from the Window menu or the palette).
    pub fn close(&mut self, panel: &PanelId) -> bool {
        self.remove_quiet(panel)
    }

    /// Make `panel` the shown tab of its group.
    pub fn activate(&mut self, panel: &PanelId) -> bool {
        let Some((area, path)) = self.find(panel) else {
            return false;
        };
        if let Some(DockNode::Tabs { panels, active }) =
            self.root_mut(area).and_then(|r| r.at_mut(&path))
            && let Some(i) = panels.iter().position(|p| p == panel)
        {
            *active = i;
            return true;
        }
        false
    }

    /// Move `panel` to index `to` within its tab group.
    pub fn reorder(&mut self, panel: &PanelId, to: usize) -> bool {
        let Some((area, path)) = self.find(panel) else {
            return false;
        };
        if let Some(DockNode::Tabs { panels, active }) =
            self.root_mut(area).and_then(|r| r.at_mut(&path))
            && let Some(i) = panels.iter().position(|p| p == panel)
        {
            let p = panels.remove(i);
            let to = to.min(panels.len());
            panels.insert(to, p);
            *active = to;
            return true;
        }
        false
    }

    /// Tear `panel` off into a new floating window at `rect`. Returns the window's id.
    pub fn float(
        &mut self,
        panel: &PanelId,
        rect: Rect,
        monitor: Option<String>,
    ) -> Result<u64, UiError> {
        if !self.contains(panel) {
            return Err(dock_err(format!("{panel} is not in the layout")));
        }
        self.remove_quiet(panel);
        let id = self.floating.iter().map(|w| w.id).max().unwrap_or(0) + 1;
        self.floating.push(FloatingWindow {
            id,
            rect,
            monitor,
            root: DockNode::Tabs {
                panels: vec![panel.clone()],
                active: 0,
            },
        });
        if self.maximised.as_ref() == Some(panel) {
            self.maximised = None;
        }
        self.normalize();
        Ok(id)
    }

    /// Put every panel of floating window `id` back into the main window (as tabs of the
    /// main window's first group), and drop the window. Closing a floating window never
    /// loses its panels.
    pub fn dock_back(&mut self, id: u64) -> bool {
        let Some(i) = self.floating.iter().position(|w| w.id == id) else {
            return false;
        };
        let w = self.floating.remove(i);
        for p in w.root.panels() {
            let target = match self.main.panels().first() {
                Some(anchor) => DropTarget::Group {
                    area: AreaId::Main,
                    anchor: anchor.clone(),
                    zone: DropZone::Tab(None),
                },
                None => DropTarget::EmptyArea(AreaId::Main),
            };
            // `insert` cannot fail here: the panel left the layout with its window.
            let _ = self.insert(p, &target);
        }
        self.normalize();
        true
    }

    /// Maximise `panel` (Shift+Space), or restore if it is maximised already.
    pub fn toggle_maximise(&mut self, panel: &PanelId) -> bool {
        if !self.contains(panel) {
            return false;
        }
        if self.maximised.as_ref() == Some(panel) {
            self.maximised = None;
        } else {
            self.maximised = Some(panel.clone());
            self.activate(panel);
        }
        true
    }

    /// Set the ratios of the split at `path` in `area` (a splitter drag).
    pub fn set_ratios(
        &mut self,
        area: AreaId,
        path: &[usize],
        ratios: Vec<f32>,
    ) -> Result<(), UiError> {
        let node = self
            .root_mut(area)
            .and_then(|r| r.at_mut(path))
            .ok_or_else(|| dock_err(format!("no split at {path:?} in {area:?}")))?;
        match node {
            DockNode::Split {
                ratios: rs,
                children,
                ..
            } if ratios.len() == children.len() => {
                *rs = normalize_ratios(ratios);
                Ok(())
            }
            _ => Err(dock_err(format!(
                "{path:?} in {area:?} is not a split of {} children",
                ratios.len()
            ))),
        }
    }

    /// Move or resize floating window `id` (the OS window moved; its monitor changed).
    pub fn set_floating_rect(&mut self, id: u64, rect: Rect, monitor: Option<String>) -> bool {
        match self.floating.iter_mut().find(|w| w.id == id) {
            Some(w) => {
                w.rect = rect;
                w.monitor = monitor;
                true
            }
            None => false,
        }
    }

    /// Replace one area's tree (a dock area edited its own tree in place).
    pub fn set_root(&mut self, area: AreaId, root: DockNode) -> bool {
        match self.root_mut(area) {
            Some(r) => {
                *r = root;
                self.normalize();
                true
            }
            None => false,
        }
    }
}

fn dedupe(node: &mut DockNode, seen: &mut std::collections::BTreeSet<PanelId>) {
    match node {
        DockNode::Tabs { panels, .. } => panels.retain(|p| seen.insert(p.clone())),
        DockNode::Split { children, .. } => {
            for c in children {
                dedupe(c, seen);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Layout {
        Layout::new(DockNode::split(
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
                (0.2, DockNode::tabs(&["forge.inspector"])),
            ],
        ))
    }

    #[test]
    fn normalisation_is_idempotent_and_flattens() {
        let l = three();
        let mut again = l.clone();
        again.normalize();
        assert_eq!(l, again);
        // A nested same-axis split melts into its parent.
        let n = DockNode::split(
            Axis::Horizontal,
            vec![
                (0.5, DockNode::tabs(&["a"])),
                (
                    0.5,
                    DockNode::split(
                        Axis::Horizontal,
                        vec![(0.5, DockNode::tabs(&["b"])), (0.5, DockNode::tabs(&["c"]))],
                    ),
                ),
            ],
        )
        .normalized();
        match n {
            Some(DockNode::Split {
                ratios, children, ..
            }) => {
                assert_eq!(children.len(), 3);
                assert!((ratios[1] - 0.25).abs() < 1e-6, "{ratios:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bad_ratios_are_repaired() {
        let r = normalize_ratios(vec![f32::NAN, 0.0, 5.0]);
        assert!((r.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(r.iter().all(|x| *x >= MIN_RATIO - 1e-6), "{r:?}");
    }

    #[test]
    fn moving_the_last_panel_out_of_a_group_collapses_it() {
        let mut l = three();
        let h = PanelId::new("forge.hierarchy");
        l.move_panel(
            &h,
            &DropTarget::Group {
                area: AreaId::Main,
                anchor: PanelId::new("forge.inspector"),
                zone: DropZone::Tab(None),
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        match &l.main {
            DockNode::Split { children, .. } => assert_eq!(children.len(), 2),
            other => panic!("{other:?}"),
        }
        assert_eq!(l.find(&h).map(|f| f.0), Some(AreaId::Main));
    }

    #[test]
    fn float_and_dock_back_keep_every_panel() {
        let mut l = three();
        let before = l.panels().len();
        let c = PanelId::new("forge.console");
        let id = l
            .float(
                &c,
                Rect::new(100.0, 100.0, 400.0, 300.0),
                Some("DISPLAY2".into()),
            )
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(l.find(&c).map(|f| f.0), Some(AreaId::Floating(id)));
        assert_eq!(l.panels().len(), before);
        assert!(l.dock_back(id));
        assert!(l.floating.is_empty());
        assert_eq!(l.find(&c).map(|f| f.0), Some(AreaId::Main));
        assert_eq!(l.panels().len(), before);
    }

    #[test]
    fn a_newer_layout_version_is_refused() {
        let mut l = three();
        l.version = LAYOUT_VERSION + 1;
        let text = ron::to_string(&l).unwrap_or_default();
        let e = Layout::from_ron(&text).expect_err("newer version");
        assert_eq!(e.code(), "UI-0008");
    }
}
