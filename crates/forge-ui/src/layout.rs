//! Layout: the `taffy` bridge (Ch.21 §21.6).
//!
//! One taffy node per widget. A widget's [`NodeStyle::layout`] is a `taffy::Style` (flex,
//! grid, block, absolute). Leaves that report `measured()` get a measure function; the
//! text measure reads the cached shaped buffer and never shapes again just to measure.
//! Layout runs in **logical pixels**; edges snap to physical pixels only at paint time.
//!
//! **Incremental layout.** A `LAYOUT`-dirty widget marks its taffy node dirty; taffy
//! propagates to ancestors and its per-node cache skips every clean subtree, so a change
//! inside a fixed-size panel never lays out its siblings.

use taffy::prelude::*;

use crate::style::{ColorRole, Radius};

/// How a widget participates in focus traversal (§21.9).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusScope {
    /// Panels and popovers: Tab stays inside while focus is inside; F6 cycles between them.
    Panel,
    /// A modal traps focus: nothing outside it is reachable while it is shown.
    Modal,
}

/// Everything about a node that is not its widget.
#[derive(Clone, Debug)]
pub struct NodeStyle {
    pub layout: Style,
    /// `false`: pointer events pass through to what is below.
    pub hit_test: bool,
    /// Children are clipped to this node's rect.
    pub clip: bool,
    pub scope: Option<FocusScope>,
    /// The background this node paints, inherited by descendants for contrast checks.
    pub background: Option<ColorRole>,
    /// Corner radius of the background (rounded panels).
    pub radius: Option<Radius>,
    /// A scroll viewport: children are offset by its scroll state and clipped.
    pub scroll: bool,
}

impl Default for NodeStyle {
    fn default() -> Self {
        Self {
            layout: Style::default(),
            hit_test: true,
            clip: false,
            scope: None,
            background: None,
            radius: None,
            scroll: false,
        }
    }
}

impl NodeStyle {
    /// A flex row with `gap` between children.
    pub fn row(gap: f32) -> Self {
        Self {
            layout: Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::CENTER),
                gap: Size {
                    width: length(gap),
                    height: length(gap),
                },
                ..Style::default()
            },
            ..Self::default()
        }
    }

    /// A flex column with `gap` between children.
    pub fn column(gap: f32) -> Self {
        Self {
            layout: Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: Size {
                    width: length(gap),
                    height: length(gap),
                },
                ..Style::default()
            },
            ..Self::default()
        }
    }

    /// A CSS grid of `cols` equal columns with `gap` between cells (the catalogue's
    /// "grid" container, §21.16).
    pub fn grid(cols: u16, gap: f32) -> Self {
        Self {
            layout: Style {
                display: Display::Grid,
                grid_template_columns: vec![fr(1.0); usize::from(cols.max(1))],
                gap: Size {
                    width: length(gap),
                    height: length(gap),
                },
                ..Style::default()
            },
            ..Self::default()
        }
    }

    /// A card: a raised, rounded, padded column (§21.16).
    pub fn card(gap: f32, pad: f32) -> Self {
        Self::column(gap)
            .padding(pad)
            .background(ColorRole::BgRaised)
            .rounded(Radius::Lg)
    }

    /// A leaf sized by its measure function.
    pub fn leaf() -> Self {
        Self::default()
    }

    pub fn padding(mut self, p: f32) -> Self {
        self.layout.padding = Rect {
            left: length(p),
            right: length(p),
            top: length(p),
            bottom: length(p),
        };
        self
    }
    /// Extra space before the content on the leading edge (nested rows).
    pub fn indent(mut self, px: f32) -> Self {
        self.layout.margin.left = length(px);
        self
    }
    pub fn size(mut self, w: f32, h: f32) -> Self {
        self.layout.size = Size {
            width: length(w),
            height: length(h),
        };
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.layout.size.width = length(w);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.layout.size.height = length(h);
        self
    }
    pub fn min_size(mut self, w: f32, h: f32) -> Self {
        self.layout.min_size = Size {
            width: length(w),
            height: length(h),
        };
        self
    }
    pub fn fill(mut self) -> Self {
        self.layout.size = Size {
            width: percent(1.0),
            height: percent(1.0),
        };
        self
    }
    pub fn grow(mut self, g: f32) -> Self {
        self.layout.flex_grow = g;
        self
    }
    pub fn wrap(mut self) -> Self {
        self.layout.flex_wrap = FlexWrap::Wrap;
        self
    }
    pub fn align_start(mut self) -> Self {
        self.layout.align_items = Some(AlignItems::START);
        self
    }
    pub fn background(mut self, role: ColorRole) -> Self {
        self.background = Some(role);
        self
    }
    /// A scroll viewport (children overflow and are clipped; see `Ui::set_scroll`).
    pub fn scrollable(mut self) -> Self {
        self.scroll = true;
        self.clip = true;
        self.layout.overflow = taffy::Point {
            x: taffy::Overflow::Scroll,
            y: taffy::Overflow::Scroll,
        };
        self
    }
    /// Round the background's corners.
    pub fn rounded(mut self, r: Radius) -> Self {
        self.radius = Some(r);
        self
    }
    pub fn scope(mut self, s: FocusScope) -> Self {
        self.scope = Some(s);
        self
    }
    pub fn clip(mut self) -> Self {
        self.clip = true;
        self
    }
    pub fn no_hit_test(mut self) -> Self {
        self.hit_test = false;
        self
    }

    /// Covers its parent exactly, out of the flow (absolute, inset 0): an overlay layer.
    pub fn cover() -> Self {
        let mut s = Self::default();
        s.layout.position = taffy::Position::Absolute;
        s.layout.inset = Rect {
            left: length(0.0),
            right: length(0.0),
            top: length(0.0),
            bottom: length(0.0),
        };
        s
    }

    /// Out of the flow, `left`/`top` logical px from its parent's corner (`None`: auto),
    /// and `right`/`bottom` likewise: `absolute(Some(8.0), None, None, Some(8.0))` sits in
    /// the bottom-left corner.
    pub fn absolute(
        left: Option<f32>,
        top: Option<f32>,
        right: Option<f32>,
        bottom: Option<f32>,
    ) -> Self {
        let at = |v: Option<f32>| v.map_or_else(auto, length);
        let mut s = Self::default();
        s.layout.position = taffy::Position::Absolute;
        s.layout.inset = Rect {
            left: at(left),
            right: at(right),
            top: at(top),
            bottom: at(bottom),
        };
        s
    }
}
