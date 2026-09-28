//! Dock geometry: where every group, tab strip and splitter of a dock tree sits, which
//! drop target a pointer is over, and where a floating window opens (Ch.21 §21.17).
//!
//! Positions are **fractions** of the dock area plus fixed pixel margins, so the same
//! numbers drive both the hit testing here and the `taffy` styles of the dock area's
//! children ([`abs_style`]): a window resize reflows the whole dock in the layout engine,
//! with no widget code running.

use taffy::prelude::{auto, length, percent};

use super::model::{AreaId, Axis, DockNode, DropTarget, DropZone, PanelId, Side};
use crate::geom::{Point, Rect};
use crate::layout::NodeStyle;

/// Height of a group's tab strip (logical px).
pub const STRIP_H: f32 = 28.0;
/// Thickness of a splitter handle: the gap between neighbouring groups (logical px).
pub const HANDLE: f32 = 6.0;
/// Width of the band along a dock area's edge that docks along the whole edge.
pub const EDGE_ZONE: f32 = 24.0;
/// A group body's side band (fraction of its size) that splits it instead of tabbing.
pub const SIDE_ZONE: f32 = 0.25;

/// A rectangle as fractions `[0, 1]` of the dock area.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Frac {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Frac {
    pub const FULL: Frac = Frac {
        x0: 0.0,
        y0: 0.0,
        x1: 1.0,
        y1: 1.0,
    };

    /// Pixel margins `[left, top, right, bottom]`: half a handle on every interior side.
    pub fn margins(&self) -> [f32; 4] {
        let h = HANDLE * 0.5;
        let inner = |f: f32| f > 1e-4 && f < 1.0 - 1e-4;
        [
            if inner(self.x0) { h } else { 0.0 },
            if inner(self.y0) { h } else { 0.0 },
            if inner(self.x1) { h } else { 0.0 },
            if inner(self.y1) { h } else { 0.0 },
        ]
    }

    /// The rect in pixels inside `area`, margins applied.
    pub fn rect_in(&self, area: Rect) -> Rect {
        let [ml, mt, mr, mb] = self.margins();
        let x0 = area.x + area.w * self.x0 + ml;
        let y0 = area.y + area.h * self.y0 + mt;
        let x1 = area.x + area.w * self.x1 - mr;
        let y1 = area.y + area.h * self.y1 - mb;
        Rect::from_min_max(x0, y0, x1.max(x0), y1.max(y0))
    }
}

/// One tab group of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupGeom {
    /// Its path in the tree.
    pub path: Vec<usize>,
    pub frac: Frac,
    pub panels: Vec<PanelId>,
    pub active: usize,
}

impl GroupGeom {
    /// The tab strip and the body (panel) rects inside `area`.
    pub fn rects(&self, area: Rect) -> (Rect, Rect) {
        let r = self.frac.rect_in(area);
        let strip = Rect::new(r.x, r.y, r.w, STRIP_H.min(r.h));
        let body = Rect::new(r.x, r.y + strip.h, r.w, (r.h - strip.h).max(0.0));
        (strip, body)
    }

    pub fn active_panel(&self) -> Option<&PanelId> {
        self.panels
            .get(self.active.min(self.panels.len().saturating_sub(1)))
    }
}

/// One splitter handle: between child `index` and `index + 1` of the split at `path`.
#[derive(Clone, Debug, PartialEq)]
pub struct HandleGeom {
    pub path: Vec<usize>,
    pub index: usize,
    pub axis: Axis,
    /// The split's own rect (fractions).
    pub split: Frac,
    /// The boundary's position along the axis (a fraction of the area).
    pub at: f32,
    /// The split's ratios now (a drag rewrites the pair around `index`).
    pub ratios: Vec<f32>,
}

impl HandleGeom {
    /// The handle's rect in pixels inside `area`.
    pub fn rect_in(&self, area: Rect) -> Rect {
        let s = self.split.rect_in(area);
        match self.axis {
            Axis::Horizontal => {
                let x = area.x + area.w * self.at;
                Rect::new(x - HANDLE * 0.5, s.y, HANDLE, s.h)
            }
            Axis::Vertical => {
                let y = area.y + area.h * self.at;
                Rect::new(s.x, y - HANDLE * 0.5, s.w, HANDLE)
            }
        }
    }

    /// The ratios after dragging this handle to `p` inside `area`, keeping every other
    /// child's share and at least the minimum share on both sides.
    pub fn drag_to(&self, area: Rect, p: Point) -> Vec<f32> {
        let (a0, a1, pos) = match self.axis {
            Axis::Horizontal => (
                area.x + area.w * self.split.x0,
                area.x + area.w * self.split.x1,
                p.x,
            ),
            Axis::Vertical => (
                area.y + area.h * self.split.y0,
                area.y + area.h * self.split.y1,
                p.y,
            ),
        };
        let span = (a1 - a0).max(1.0);
        let t = ((pos - a0) / span).clamp(0.0, 1.0);
        self.set_boundary(t)
    }

    /// The ratios with this boundary moved to `t` (a fraction of the split).
    pub fn set_boundary(&self, t: f32) -> Vec<f32> {
        let mut r = self.ratios.clone();
        let (i, j) = (self.index, self.index + 1);
        if j >= r.len() {
            return r;
        }
        let before: f32 = r[..i].iter().sum();
        let pair = r[i] + r[j];
        let min = super::model::MIN_RATIO;
        let left = (t - before).clamp(min, (pair - min).max(min));
        r[i] = left;
        r[j] = (pair - left).max(min);
        super::model::normalize_ratios(r)
    }
}

/// Every group and handle of a tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    pub groups: Vec<GroupGeom>,
    pub handles: Vec<HandleGeom>,
}

/// Lay a tree out in fractions. With `maximised` in this tree, its group fills the area
/// alone and no handle is shown.
pub fn geometry(root: &DockNode, maximised: Option<&PanelId>) -> Geometry {
    let mut g = Geometry::default();
    if let Some(m) = maximised
        && let Some(path) = root.group_path(m)
        && let Some(DockNode::Tabs { panels, .. }) = root.at(&path)
    {
        let active = panels.iter().position(|p| p == m).unwrap_or(0);
        g.groups.push(GroupGeom {
            path,
            frac: Frac::FULL,
            panels: panels.clone(),
            active,
        });
        return g;
    }
    walk(root, Frac::FULL, &mut Vec::new(), &mut g);
    g
}

fn walk(node: &DockNode, f: Frac, path: &mut Vec<usize>, g: &mut Geometry) {
    match node {
        DockNode::Tabs { panels, active } => {
            if !panels.is_empty() {
                g.groups.push(GroupGeom {
                    path: path.clone(),
                    frac: f,
                    panels: panels.clone(),
                    active: (*active).min(panels.len() - 1),
                });
            }
        }
        DockNode::Split {
            axis,
            ratios,
            children,
        } => {
            let sum: f32 = ratios.iter().sum::<f32>().max(f32::EPSILON);
            let mut cursor = 0.0f32;
            for (i, (c, r)) in children.iter().zip(ratios).enumerate() {
                let a = cursor;
                let b = cursor + r / sum;
                cursor = b;
                let cf = match axis {
                    Axis::Horizontal => Frac {
                        x0: f.x0 + (f.x1 - f.x0) * a,
                        x1: f.x0 + (f.x1 - f.x0) * b,
                        ..f
                    },
                    Axis::Vertical => Frac {
                        y0: f.y0 + (f.y1 - f.y0) * a,
                        y1: f.y0 + (f.y1 - f.y0) * b,
                        ..f
                    },
                };
                path.push(i);
                walk(c, cf, path, g);
                path.pop();
                if i + 1 < children.len() {
                    g.handles.push(HandleGeom {
                        path: path.clone(),
                        index: i,
                        axis: *axis,
                        split: f,
                        at: match axis {
                            Axis::Horizontal => cf.x1,
                            Axis::Vertical => cf.y1,
                        },
                        ratios: ratios.clone(),
                    });
                }
            }
        }
    }
}

/// An absolutely positioned child of the dock area at `frac` (percent insets) with pixel
/// `margins`, and optionally a fixed pixel `height` from its top.
pub fn abs_style(frac: Frac, margins: [f32; 4], height: Option<f32>) -> NodeStyle {
    let mut s = NodeStyle::default();
    s.layout.position = taffy::Position::Absolute;
    s.layout.inset = taffy::Rect {
        left: percent(frac.x0),
        right: percent(1.0 - frac.x1),
        top: percent(frac.y0),
        bottom: match height {
            Some(_) => auto(),
            None => percent(1.0 - frac.y1),
        },
    };
    s.layout.margin = taffy::Rect {
        left: length(margins[0]),
        top: length(margins[1]),
        right: length(margins[2]),
        bottom: length(margins[3]),
    };
    if let Some(h) = height {
        s.layout.size.height = length(h);
    }
    s
}

/// A drop target under the pointer, with the preview rect to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct DropHit {
    pub target: DropTarget,
    /// Where the panel would go (logical px, window coordinates).
    pub preview: Rect,
}

/// The drop target at `p` in a dock area showing `geom` in `area`, for dragging `dragged`.
/// `tab_at(group, x)` gives the insertion index for a pointer over a group's tab strip.
/// `None`: nothing to drop onto here (outside, or a drop that would change nothing).
pub fn drop_at(
    area_id: AreaId,
    geom: &Geometry,
    area: Rect,
    p: Point,
    dragged: &PanelId,
    tab_at: &dyn Fn(usize, f32) -> Option<usize>,
) -> Option<DropHit> {
    if !area.contains(p) {
        return None;
    }
    if geom.groups.is_empty() {
        return Some(DropHit {
            target: DropTarget::EmptyArea(area_id),
            preview: area,
        });
    }
    // A tab strip wins over the area's edge band (the strips of the top groups lie in it).
    let over_strip = geom.groups.iter().any(|g| g.rects(area).0.contains(p));
    // The area's own edges: dock along the whole side.
    let edges = [
        (p.x - area.x, Side::Left),
        (area.right() - p.x, Side::Right),
        (p.y - area.y, Side::Top),
        (area.bottom() - p.y, Side::Bottom),
    ];
    if let Some((_, side)) = edges
        .iter()
        .filter(|(d, _)| *d < EDGE_ZONE && !over_strip)
        .min_by(|a, b| a.0.total_cmp(&b.0))
    {
        let only_me = geom.groups.len() == 1 && geom.groups[0].panels == [dragged.clone()];
        if only_me {
            return None;
        }
        return Some(DropHit {
            target: DropTarget::AreaEdge {
                area: area_id,
                side: *side,
            },
            preview: side_band(area, *side, 0.25),
        });
    }
    let (gi, g) = geom
        .groups
        .iter()
        .enumerate()
        .find(|(_, g)| g.frac.rect_in(area).contains(p))?;
    let alone = g.panels.len() == 1 && g.panels[0] == *dragged;
    let anchor = g.panels.iter().find(|q| *q != dragged).cloned();
    let (strip, body) = g.rects(area);
    let zone = if strip.contains(p) {
        DropZone::Tab(tab_at(gi, p.x))
    } else {
        let u = ((p.x - body.x) / body.w.max(1.0)).clamp(0.0, 1.0);
        let v = ((p.y - body.y) / body.h.max(1.0)).clamp(0.0, 1.0);
        let sides = [
            (u, Side::Left),
            (1.0 - u, Side::Right),
            (v, Side::Top),
            (1.0 - v, Side::Bottom),
        ];
        match sides
            .iter()
            .filter(|(d, _)| *d < SIDE_ZONE)
            .min_by(|a, b| a.0.total_cmp(&b.0))
        {
            Some((_, side)) => DropZone::Side(*side),
            None => DropZone::Tab(None),
        }
    };
    if alone || (zone == DropZone::Tab(None) && g.panels.contains(dragged)) {
        // Onto its own single-panel group, or the body of its own group: nothing changes.
        return None;
    }
    let anchor = match zone {
        // Within its own group a tab move re-anchors on itself (a reorder)...
        DropZone::Tab(_) if g.panels.contains(dragged) => dragged.clone(),
        // ...and a side split of its own group anchors on a sibling.
        _ => anchor.unwrap_or_else(|| dragged.clone()),
    };
    let preview = match zone {
        DropZone::Tab(_) => body,
        DropZone::Side(s) => side_band(body, s, 0.5),
    };
    Some(DropHit {
        target: DropTarget::Group {
            area: area_id,
            anchor,
            zone,
        },
        preview,
    })
}

/// The band of `r` along `side` taking `share` of it.
pub fn side_band(r: Rect, side: Side, share: f32) -> Rect {
    match side {
        Side::Left => Rect::new(r.x, r.y, r.w * share, r.h),
        Side::Right => Rect::new(r.right() - r.w * share, r.y, r.w * share, r.h),
        Side::Top => Rect::new(r.x, r.y, r.w, r.h * share),
        Side::Bottom => Rect::new(r.x, r.bottom() - r.h * share, r.w, r.h * share),
    }
}

/// A monitor as the platform reports it (logical px of the virtual screen).
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub name: Option<String>,
    pub rect: Rect,
    pub primary: bool,
}

/// Where a floating window opens: its saved rect if it is still reachable on some monitor
/// (at least a title bar's worth visible); otherwise centred on its hinted monitor if that
/// still exists, else on the primary one — never off-screen because a monitor was
/// unplugged. Returns the rect and the monitor it lands on.
pub fn place_window(
    rect: Rect,
    hint: Option<&str>,
    monitors: &[MonitorInfo],
) -> (Rect, Option<String>) {
    if monitors.is_empty() {
        return (rect, hint.map(str::to_string));
    }
    let title = Rect::new(rect.x, rect.y, rect.w, 32.0_f32.min(rect.h));
    let visible = |m: &MonitorInfo| {
        let i = title.intersect(&m.rect);
        i.w >= 48.0 && i.h >= 16.0
    };
    if let Some(m) = monitors.iter().filter(|m| visible(m)).max_by(|a, b| {
        rect.intersect(&a.rect)
            .area()
            .total_cmp(&rect.intersect(&b.rect).area())
    }) {
        return (rect, m.name.clone());
    }
    let m = hint
        .and_then(|h| monitors.iter().find(|m| m.name.as_deref() == Some(h)))
        .or_else(|| monitors.iter().find(|m| m.primary))
        .unwrap_or(&monitors[0]);
    let w = rect.w.min(m.rect.w);
    let h = rect.h.min(m.rect.h);
    let r = Rect::new(
        m.rect.x + (m.rect.w - w) * 0.5,
        m.rect.y + (m.rect.h - h) * 0.5,
        w,
        h,
    );
    (r, m.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> DockNode {
        DockNode::split(
            Axis::Horizontal,
            vec![
                (0.25, DockNode::tabs(&["a"])),
                (0.75, DockNode::tabs(&["b", "c"])),
            ],
        )
    }

    #[test]
    fn groups_and_handles_tile_the_area() {
        let g = geometry(&two(), None);
        assert_eq!(g.groups.len(), 2);
        assert_eq!(g.handles.len(), 1);
        assert!((g.handles[0].at - 0.25).abs() < 1e-6);
        let area = Rect::new(0.0, 0.0, 800.0, 600.0);
        let a = g.groups[0].frac.rect_in(area);
        let b = g.groups[1].frac.rect_in(area);
        assert!((b.x - a.right() - HANDLE).abs() < 1e-3, "{a:?} {b:?}");
        let h = g.handles[0].rect_in(area);
        assert!((h.x - a.right()).abs() < 1e-3 && (h.right() - b.x).abs() < 1e-3);
    }

    #[test]
    fn maximised_fills_the_area_alone() {
        let g = geometry(&two(), Some(&PanelId::new("c")));
        assert_eq!(g.groups.len(), 1);
        assert!(g.handles.is_empty());
        assert_eq!(g.groups[0].active_panel(), Some(&PanelId::new("c")));
    }

    #[test]
    fn drop_zones_follow_the_pointer() {
        let root = two();
        let g = geometry(&root, None);
        let area = Rect::new(0.0, 0.0, 800.0, 600.0);
        let d = PanelId::new("a");
        let hit = |x, y| {
            drop_at(AreaId::Main, &g, area, Point::new(x, y), &d, &|_, _| {
                Some(0)
            })
        };
        // Centre of b's body: a tab.
        let t = hit(500.0, 330.0).map(|h| h.target);
        assert!(
            matches!(
                t,
                Some(DropTarget::Group {
                    zone: DropZone::Tab(None),
                    ..
                })
            ),
            "{t:?}"
        );
        // Right band of b's body: a split on the right.
        let t = hit(780.0 - EDGE_ZONE, 330.0).map(|h| h.target);
        assert!(
            matches!(
                t,
                Some(DropTarget::Group {
                    zone: DropZone::Side(Side::Right),
                    ..
                })
            ),
            "{t:?}"
        );
        // The window edge: dock along the whole area.
        let t = hit(795.0, 330.0).map(|h| h.target);
        assert!(
            matches!(
                t,
                Some(DropTarget::AreaEdge {
                    side: Side::Right,
                    ..
                })
            ),
            "{t:?}"
        );
        // Over b's tab strip: a tab at the index the strip reports.
        let t = hit(500.0, 10.0).map(|h| h.target);
        assert!(
            matches!(
                t,
                Some(DropTarget::Group {
                    zone: DropZone::Tab(Some(0)),
                    ..
                })
            ),
            "{t:?}"
        );
        // Onto its own single-panel group: nothing.
        assert!(hit(100.0, 300.0).is_none());
    }

    #[test]
    fn handle_drag_moves_only_its_boundary() {
        let root = DockNode::split(
            Axis::Horizontal,
            vec![
                (0.2, DockNode::tabs(&["a"])),
                (0.5, DockNode::tabs(&["b"])),
                (0.3, DockNode::tabs(&["c"])),
            ],
        );
        let g = geometry(&root, None);
        let area = Rect::new(0.0, 0.0, 1000.0, 500.0);
        let r = g.handles[0].drag_to(area, Point::new(400.0, 10.0));
        assert!(
            (r[0] - 0.4).abs() < 1e-4 && (r[1] - 0.3).abs() < 1e-4,
            "{r:?}"
        );
        assert!(
            (r[2] - 0.3).abs() < 1e-4,
            "the third child keeps its share: {r:?}"
        );
    }

    #[test]
    fn a_window_on_an_unplugged_monitor_reopens_on_screen() {
        let mons = vec![
            MonitorInfo {
                name: Some("DISPLAY1".into()),
                rect: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                primary: true,
            },
            MonitorInfo {
                name: Some("DISPLAY2".into()),
                rect: Rect::new(1920.0, 0.0, 2560.0, 1440.0),
                primary: false,
            },
        ];
        // Still visible on DISPLAY2: kept where it was.
        let (r, m) = place_window(Rect::new(2100.0, 200.0, 600.0, 400.0), None, &mons);
        assert_eq!((r.x, m.as_deref()), (2100.0, Some("DISPLAY2")));
        // Saved on a third monitor that is gone: centred on the primary.
        let (r, m) = place_window(
            Rect::new(5000.0, 200.0, 600.0, 400.0),
            Some("DISPLAY3"),
            &mons,
        );
        assert_eq!(m.as_deref(), Some("DISPLAY1"));
        assert!(
            Rect::new(0.0, 0.0, 1920.0, 1080.0).contains_rect(&r),
            "{r:?}"
        );
        // Off-screen but its hinted monitor exists: centred there.
        let (_, m) = place_window(
            Rect::new(-9000.0, 0.0, 600.0, 400.0),
            Some("DISPLAY2"),
            &mons,
        );
        assert_eq!(m.as_deref(), Some("DISPLAY2"));
    }
}
