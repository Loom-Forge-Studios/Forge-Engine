//! Focus traversal and spatial navigation (Ch.21 §21.9).
//!
//! Pure functions over the focusable widgets of the current scope, so the traversal
//! rules are unit-testable without a tree.
//!
//! * **Tab / Shift+Tab** traverse in **visual order**: rows top to bottom (two widgets
//!   share a row when their vertical extents overlap by more than half the shorter one),
//!   left to right within a row.
//! * **Spatial navigation** (the gamepad fallback, §21.20) picks the nearest focusable
//!   widget in the pressed direction, weighted by overlap on the cross axis.

use crate::geom::Rect;
use crate::id::WidgetId;

/// A focusable widget as traversal sees it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Focusable {
    pub id: WidgetId,
    pub rect: Rect,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

fn same_row(a: &Rect, b: &Rect) -> bool {
    let overlap = a.bottom().min(b.bottom()) - a.y.max(b.y);
    overlap > 0.5 * a.h.min(b.h)
}

/// Sort into visual (reading) order.
pub fn visual_order(mut items: Vec<Focusable>) -> Vec<Focusable> {
    items.sort_by(|a, b| a.rect.y.total_cmp(&b.rect.y));
    // Group into rows, then order each row left to right.
    let mut rows: Vec<Vec<Focusable>> = Vec::new();
    for it in items {
        match rows.last_mut() {
            Some(row) if row.iter().any(|r| same_row(&r.rect, &it.rect)) => row.push(it),
            _ => rows.push(vec![it]),
        }
    }
    rows.into_iter()
        .flat_map(|mut r| {
            r.sort_by(|a, b| a.rect.x.total_cmp(&b.rect.x));
            r
        })
        .collect()
}

/// The next widget in visual order after `current` (wrapping), or the first.
pub fn next(order: &[Focusable], current: Option<WidgetId>, backward: bool) -> Option<WidgetId> {
    if order.is_empty() {
        return None;
    }
    let pos = current.and_then(|c| order.iter().position(|f| f.id == c));
    let n = order.len();
    let i = match (pos, backward) {
        (None, false) => 0,
        (None, true) => n - 1,
        (Some(p), false) => (p + 1) % n,
        (Some(p), true) => (p + n - 1) % n,
    };
    Some(order[i].id)
}

/// Spatial navigation: the nearest candidate strictly in `dir` from `from`, scored by
/// distance along the axis plus a penalty for poor cross-axis overlap.
pub fn spatial(items: &[Focusable], from: Rect, dir: Direction) -> Option<WidgetId> {
    let c = from.center();
    items
        .iter()
        .filter_map(|f| {
            let r = f.rect;
            let rc = r.center();
            let (along, ahead, overlap, cross_len) = match dir {
                Direction::Right => (
                    r.x - from.right(),
                    rc.x > c.x + 0.5,
                    overlap(from.y, from.bottom(), r.y, r.bottom()),
                    from.h.min(r.h),
                ),
                Direction::Left => (
                    from.x - r.right(),
                    rc.x < c.x - 0.5,
                    overlap(from.y, from.bottom(), r.y, r.bottom()),
                    from.h.min(r.h),
                ),
                Direction::Down => (
                    r.y - from.bottom(),
                    rc.y > c.y + 0.5,
                    overlap(from.x, from.right(), r.x, r.right()),
                    from.w.min(r.w),
                ),
                Direction::Up => (
                    from.y - r.bottom(),
                    rc.y < c.y - 0.5,
                    overlap(from.x, from.right(), r.x, r.right()),
                    from.w.min(r.w),
                ),
            };
            if !ahead {
                return None;
            }
            let cross = match dir {
                Direction::Left | Direction::Right => (rc.y - c.y).abs(),
                Direction::Up | Direction::Down => (rc.x - c.x).abs(),
            };
            let overlap_frac = if cross_len > 0.0 {
                (overlap / cross_len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // Candidates in the beam (overlapping the source on the cross axis) always win,
            // nearest along the axis first: Down from a wide slider goes to the next row,
            // not to a nearer-centred control two rows down. Out of the beam, distance
            // along the axis plus a cross-axis penalty.
            let in_beam = overlap > 0.0;
            let score = if in_beam {
                along.max(0.0) + cross * 0.01
            } else {
                along.max(0.0) + cross * (2.0 - overlap_frac) * 2.0
            };
            Some((!in_beam, score, f.id))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
        .map(|(_, _, id)| id)
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: u64, x: f32, y: f32) -> Focusable {
        Focusable {
            id: WidgetId(id),
            rect: Rect::new(x, y, 80.0, 24.0),
        }
    }

    #[test]
    fn visual_order_is_rows_then_columns() {
        // Inserted out of order; row 1 is slightly misaligned vertically.
        let items = vec![
            f(4, 100.0, 40.0),
            f(1, 0.0, 2.0),
            f(3, 0.0, 40.0),
            f(2, 100.0, 0.0),
        ];
        let ids: Vec<u64> = visual_order(items).iter().map(|f| f.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn next_wraps_both_ways() {
        let o = visual_order(vec![f(1, 0.0, 0.0), f(2, 100.0, 0.0)]);
        assert_eq!(next(&o, Some(WidgetId(2)), false), Some(WidgetId(1)));
        assert_eq!(next(&o, Some(WidgetId(1)), true), Some(WidgetId(2)));
        assert_eq!(next(&o, None, false), Some(WidgetId(1)));
    }

    #[test]
    fn spatial_goes_to_the_next_row_in_the_beam() {
        // A wide control, then a narrow one at its left edge, then a mid-width one whose
        // centre is closer: Down goes to the next row (in the beam), not the nearer centre.
        let wide = Rect::new(0.0, 0.0, 300.0, 24.0);
        let items = vec![
            Focusable {
                id: WidgetId(2),
                rect: Rect::new(0.0, 40.0, 60.0, 24.0),
            },
            Focusable {
                id: WidgetId(3),
                rect: Rect::new(0.0, 120.0, 220.0, 24.0),
            },
        ];
        assert_eq!(spatial(&items, wide, Direction::Down), Some(WidgetId(2)));
        assert_eq!(
            spatial(&items, Rect::new(0.0, 40.0, 60.0, 24.0), Direction::Up),
            None,
            "nothing above"
        );
    }

    #[test]
    fn spatial_prefers_overlap() {
        let items = vec![f(2, 200.0, 0.0), f(3, 120.0, 60.0)];
        assert_eq!(
            spatial(&items, Rect::new(0.0, 0.0, 80.0, 24.0), Direction::Right),
            Some(WidgetId(2))
        );
        assert_eq!(
            spatial(&items, Rect::new(0.0, 0.0, 80.0, 24.0), Direction::Down),
            Some(WidgetId(3))
        );
    }
}
