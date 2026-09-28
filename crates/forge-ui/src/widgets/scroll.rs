//! Scroll area and splitter (§21.16).
//!
//! * **Scroll area**: a viewport (`NodeStyle::scrollable`) plus a scrollbar. The wheel
//!   anywhere inside scrolls it (trackpads deliver their own momentum: "kinetic where the
//!   platform does it"); focused, it scrolls with Up/Down, PageUp/PageDown, Home/End; the
//!   thumb drags; and keyboard focus moving onto a widget scrolled out of view brings it
//!   into view (`Ui::scroll_into_view`). Scrolling moves rects only — no taffy pass —
//!   and re-records only the slices that moved.
//! * **Splitter**: two panes and a draggable handle; the handle is a tab stop (arrows move
//!   it 2 %, PageUp/PageDown 10 %, Home/End to the limits) and an AccessKit splitter with
//!   its position as value. The ratio lives in a signal (saved with the layout, §21.4).

use accesskit::{Action, Orientation, Role};

use crate::UiError;
use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::layout::NodeStyle;
use crate::state::Signal;
use crate::style::ColorRole;
use crate::ui::{ScrollState, Ui};
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

const BAR: f32 = 10.0;
const LINE: f32 = 40.0;

/// The ids a scroll area is made of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScrollParts {
    /// The scroll area (focusable; handles keys and the wheel).
    pub area: WidgetId,
    /// Add content here.
    pub viewport: WidgetId,
    pub scrollbar: WidgetId,
}

/// Build a scroll area under `parent`. `style` sizes the area; content added to
/// `viewport` scrolls.
pub fn scroll_area(
    ui: &mut Ui,
    parent: WidgetId,
    key: impl Into<Key>,
    style: NodeStyle,
    label: &str,
) -> Result<ScrollParts, UiError> {
    let state = ui.rt_mut().signal(ScrollState::default());
    let mut area_style = style;
    area_style.layout.position = taffy::Position::Relative;
    let area = ui.add(parent, key, area_style, ScrollArea::new(label, state))?;
    let mut vs = NodeStyle::column(ui.theme().space[2]).scrollable();
    vs.layout.size = taffy::Size {
        width: taffy::prelude::percent(1.0),
        height: taffy::prelude::percent(1.0),
    };
    vs.layout.padding.right = taffy::prelude::length(BAR + 2.0);
    let viewport = ui.add(area, "viewport", vs, super::Container::group())?;
    let mut bs = NodeStyle::default();
    bs.layout.position = taffy::Position::Absolute;
    bs.layout.inset = taffy::Rect {
        left: taffy::prelude::auto(),
        top: taffy::prelude::length(0.0),
        right: taffy::prelude::length(0.0),
        bottom: taffy::prelude::length(0.0),
    };
    bs.layout.size.width = taffy::prelude::length(BAR);
    let scrollbar = ui.add(
        area,
        "scrollbar",
        bs,
        Scrollbar {
            state,
            viewport,
            drag: None,
        },
    )?;
    if let Some(a) = ui.widget_mut::<ScrollArea>(area) {
        a.viewport = Some(viewport);
    }
    Ok(ScrollParts {
        area,
        viewport,
        scrollbar,
    })
}

/// The scroll area's own widget.
pub struct ScrollArea {
    label: String,
    state: Signal<ScrollState>,
    viewport: Option<WidgetId>,
}

impl ScrollArea {
    fn new(label: &str, state: Signal<ScrollState>) -> Self {
        Self {
            label: label.to_string(),
            state,
            viewport: None,
        }
    }
    fn scroll_by(&self, cx: &mut EventCx, dy: f32) -> bool {
        let Some(v) = self.viewport else {
            return false;
        };
        let Some(s) = cx.scroll_state(v) else {
            return false;
        };
        let o = Point::new(s.offset.x, s.offset.y + dy);
        cx.set_scroll(v, o);
        cx.scroll_state(v).is_some_and(|n| n.offset != s.offset)
    }
}

impl Widget for ScrollArea {
    fn role(&self) -> Role {
        Role::ScrollView
    }
    fn focusable(&self) -> bool {
        true
    }
    fn hover_sensitive(&self) -> bool {
        false
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::ScrollChanged => {
                if let Some(v) = self.viewport
                    && let Some(s) = cx.scroll_state(v)
                {
                    self.state.set(cx.rt_mut(), s);
                }
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::Wheel { dy, .. } => {
                if self.scroll_by(cx, -dy) {
                    Handled::Yes
                } else {
                    Handled::No
                }
            }
            UiEvent::Key(k) if k.pressed => {
                let Some(v) = self.viewport else {
                    return Handled::No;
                };
                let Some(s) = cx.scroll_state(v) else {
                    return Handled::No;
                };
                let page = (s.viewport.h - LINE).max(LINE);
                let dy = match k.code {
                    KeyCode::Down => LINE,
                    KeyCode::Up => -LINE,
                    KeyCode::PageDown | KeyCode::Space => page,
                    KeyCode::PageUp => -page,
                    KeyCode::Home => -s.offset.y,
                    KeyCode::End => s.max_offset().y - s.offset.y,
                    _ => return Handled::No,
                };
                self.scroll_by(cx, dy);
                Handled::Yes
            }
            UiEvent::A11yAction(Action::ScrollDown) => {
                self.scroll_by(cx, LINE * 3.0);
                Handled::Yes
            }
            UiEvent::A11yAction(Action::ScrollUp) => {
                self.scroll_by(cx, -LINE * 3.0);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        let s = self.state.get(cx.rt);
        node.set_scroll_y(f64::from(s.offset.y));
        node.set_scroll_y_min(0.0);
        node.set_scroll_y_max(f64::from(s.max_offset().y));
        node.add_action(Action::ScrollDown);
        node.add_action(Action::ScrollUp);
    }
}

/// A vertical scrollbar for a viewport.
pub struct Scrollbar {
    state: Signal<ScrollState>,
    viewport: WidgetId,
    drag: Option<(f32, f32)>,
}

impl Scrollbar {
    fn thumb(&self, r: Rect, s: &ScrollState) -> Option<Rect> {
        if s.content.h <= s.viewport.h + 0.5 {
            return None;
        }
        let frac = (s.viewport.h / s.content.h).clamp(0.05, 1.0);
        let h = (r.h * frac).max(20.0).min(r.h);
        let max = s.max_offset().y.max(1.0);
        let y = r.y + (r.h - h) * (s.offset.y / max);
        Some(Rect::new(r.x + 2.0, y, r.w - 4.0, h))
    }
}

impl Widget for Scrollbar {
    fn role(&self) -> Role {
        Role::ScrollBar
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.state.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let s = self.state.get(cx.rt());
        let r = cx.rect();
        match ev {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                match self.thumb(r, &s) {
                    Some(t) if t.contains(*pos) => {
                        self.drag = Some((pos.y, s.offset.y));
                        cx.capture_pointer();
                    }
                    Some(t) => {
                        // Page towards the press.
                        let page = s.viewport.h * if pos.y < t.y { -1.0 } else { 1.0 };
                        cx.set_scroll(self.viewport, Point::new(s.offset.x, s.offset.y + page));
                    }
                    None => {}
                }
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                if let (Some((y0, o0)), Some(t)) = (self.drag, self.thumb(r, &s)) {
                    let travel = (r.h - t.h).max(1.0);
                    let o = o0 + (pos.y - y0) / travel * s.max_offset().y;
                    cx.set_scroll(self.viewport, Point::new(s.offset.x, o));
                }
                Handled::Yes
            }
            UiEvent::PointerUp { .. } => {
                if self.drag.take().is_some() {
                    cx.release_pointer();
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let s = self.state.get(cx.rt());
        if let Some(t) = self.thumb(r, &s) {
            let bg = cx.parent_bg();
            cx.mark(t, ColorRole::Border, bg, t.w * 0.5);
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        let s = self.state.get(cx.rt);
        node.set_orientation(Orientation::Vertical);
        node.set_numeric_value(f64::from(s.offset.y));
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(f64::from(s.max_offset().y));
        node.set_label(crate::tr!("Scroll position"));
    }
}

/// Split direction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Panes side by side, a vertical handle.
    Horizontal,
    /// Panes stacked, a horizontal handle.
    Vertical,
}

/// The ids a splitter is made of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SplitParts {
    pub split: WidgetId,
    pub first: WidgetId,
    pub handle: WidgetId,
    pub second: WidgetId,
}

const HANDLE: f32 = 6.0;

fn pane_style(axis: Axis, ratio: f32) -> NodeStyle {
    let mut s = NodeStyle::column(0.0).clip();
    s.layout.flex_basis = taffy::prelude::percent(ratio.clamp(0.0, 1.0));
    s.layout.flex_shrink = 1.0;
    s.layout.flex_grow = 0.0;
    match axis {
        Axis::Horizontal => s.layout.min_size.width = taffy::prelude::length(0.0),
        Axis::Vertical => s.layout.min_size.height = taffy::prelude::length(0.0),
    }
    s
}

/// Build a splitter under `parent`; `ratio` is the first pane's share (0..=1).
pub fn splitter(
    ui: &mut Ui,
    parent: WidgetId,
    key: impl Into<Key>,
    style: NodeStyle,
    axis: Axis,
    ratio: Signal<f32>,
    label: &str,
) -> Result<SplitParts, UiError> {
    let mut st = style;
    st.layout.display = taffy::Display::Flex;
    st.layout.flex_direction = match axis {
        Axis::Horizontal => taffy::FlexDirection::Row,
        Axis::Vertical => taffy::FlexDirection::Column,
    };
    st.layout.align_items = Some(taffy::AlignItems::STRETCH);
    let split = ui.add(parent, key, st, super::Container::group().labelled(label))?;
    let r = ratio.get(ui.rt());
    let first = ui.add(
        split,
        "first",
        pane_style(axis, r),
        super::Container::group(),
    )?;
    let hs = match axis {
        Axis::Horizontal => NodeStyle::leaf().width(HANDLE),
        Axis::Vertical => NodeStyle::leaf().height(HANDLE),
    };
    let handle = ui.add(
        split,
        "handle",
        hs,
        SplitHandle {
            axis,
            ratio,
            first: None,
            label: label.to_string(),
            drag: false,
        },
    )?;
    let mut ss = NodeStyle::column(0.0).clip().grow(1.0);
    ss.layout.flex_basis = taffy::prelude::length(0.0);
    let second = ui.add(split, "second", ss, super::Container::group())?;
    if let Some(h) = ui.widget_mut::<SplitHandle>(handle) {
        h.first = Some(first);
    }
    Ok(SplitParts {
        split,
        first,
        handle,
        second,
    })
}

/// The draggable handle of a splitter.
pub struct SplitHandle {
    axis: Axis,
    ratio: Signal<f32>,
    first: Option<WidgetId>,
    label: String,
    drag: bool,
}

impl SplitHandle {
    fn set(&self, cx: &mut EventCx, r: f32) {
        let r = r.clamp(0.05, 0.95);
        self.ratio.set(cx.rt_mut(), r);
        if let Some(f) = self.first {
            cx.set_style(f, pane_style(self.axis, r));
        }
    }
}

impl Widget for SplitHandle {
    fn role(&self) -> Role {
        Role::Splitter
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.ratio.any()), Dirty::PAINT | Dirty::A11Y);
    }
    fn focusable(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        _k: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(HANDLE, HANDLE)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let r = self.ratio.get(cx.rt());
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let nr = match (k.code.clone(), self.axis) {
                    (KeyCode::Right, Axis::Horizontal) | (KeyCode::Down, Axis::Vertical) => {
                        r + 0.02
                    }
                    (KeyCode::Left, Axis::Horizontal) | (KeyCode::Up, Axis::Vertical) => r - 0.02,
                    (KeyCode::PageDown, _) => r + 0.1,
                    (KeyCode::PageUp, _) => r - 0.1,
                    (KeyCode::Home, _) => 0.0,
                    (KeyCode::End, _) => 1.0,
                    _ => return Handled::No,
                };
                self.set(cx, nr);
                Handled::Yes
            }
            UiEvent::PointerDown {
                button: PointerButton::Primary,
                ..
            } => {
                self.drag = true;
                cx.capture_pointer();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } if self.drag => {
                // The split container is the handle's parent: its rect is the pane range.
                let parent = cx.ui.parent(cx.id()).and_then(|p| cx.rect_of(p));
                if let Some(pr) = parent {
                    let nr = match self.axis {
                        Axis::Horizontal => (pos.x - pr.x) / pr.w.max(1.0),
                        Axis::Vertical => (pos.y - pr.y) / pr.h.max(1.0),
                    };
                    self.set(cx, nr);
                }
                Handled::Yes
            }
            UiEvent::PointerUp { .. } if self.drag => {
                self.drag = false;
                cx.release_pointer();
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Increment) => {
                self.set(cx, r + 0.02);
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Decrement) => {
                self.set(cx, r - 0.02);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bg = cx.parent_bg();
        let line = match self.axis {
            Axis::Horizontal => Rect::new(r.x + r.w * 0.5 - 0.5, r.y, 1.0, r.h),
            Axis::Vertical => Rect::new(r.x, r.y + r.h * 0.5 - 0.5, r.w, 1.0),
        };
        let role = if cx.state().hovered || self.drag {
            ColorRole::FocusRing
        } else {
            ColorRole::Border
        };
        cx.mark(line, role, bg, 0.0);
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_orientation(match self.axis {
            Axis::Horizontal => Orientation::Vertical,
            Axis::Vertical => Orientation::Horizontal,
        });
        node.set_numeric_value(f64::from(self.ratio.get(cx.rt)) * 100.0);
        node.set_min_numeric_value(5.0);
        node.set_max_numeric_value(95.0);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
}
