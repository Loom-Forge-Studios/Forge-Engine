//! The §21.16 widget catalogue (WP-U2), used by `ui_gallery` and by the editor panels.
//!
//! Rules every widget here follows (§21.16): full keyboard operation, an AccessKit role
//! and name, theming through tokens only (no colour literal anywhere in this directory —
//! `test_ui_contrast` greps for them), and input + a11y tests (`tests/test_ui_widgets_*`).

mod button;
mod choice;
mod collections;
mod color_picker;
mod combo;
mod container;
mod curve;
mod dialog;
mod display;
mod grid;
mod integer;
mod label;
pub mod line_edit;
mod live;
mod menu;
mod multiline;
mod nav;
mod node_canvas;
mod numeric;
mod palette;
mod progress;
mod property_grid;
mod range;
mod relay;
mod rich_text;
mod scroll;
mod search;
mod slider;
mod spinner;
mod table;
mod tabs;
mod text_field;
mod toggle;
mod vector;

pub use button::{Button, ButtonSize, IconButton, Pressed};
pub use choice::{CheckState, SegmentedControl, Switch, ToggleButton, TriCheckbox};
pub use collections::{
    AssetDropped, BADGE_W, DropTarget, OVERSCAN, RowActivated, RowBadge, RowBadgeClicked,
    RowContextMenu, RowItem, RowRenamed, RowsDeleteRequested, RowsDropped, Selection,
    SelectionChanged, TYPEAHEAD_RESET, VirtualTree, live_window,
};
pub use color_picker::{
    ColorButton, ColorPicker, ColorPickerParts, ColorSurface, ColorSwatch, HexField, Surface,
    color_picker, open_color_popover,
};
pub use combo::{Autocomplete, Chips, ChoiceList, ComboBox};
pub use container::Container;
pub use curve::{
    Curve, CurveEdited, CurveEditor, CurveKey, Gradient, GradientEdited, GradientEditor,
    GradientStop, Interp,
};
pub use dialog::{DialogResult, DialogSpec, PopoverButton, PopoverPanel, open_dialog};
pub use display::{
    Badge, CollapsibleHeader, EmptyState, EmptyStateAction, GroupBox, Icon, IconView, ImageView,
    Separator, ShortcutHint, Skeleton,
};
pub use grid::{
    FilesDropped, GridMode, GridTile, GridUp, ReadyThumb, THUMB_CACHE, ThumbProvider, VirtualGrid,
};
pub use integer::{IntegerCommitted, IntegerField};
pub use label::{Label, LabelKind, VERBATIM_CLASS};
pub use line_edit::{EditOutcome, LineEdit};
pub use live::LiveReadout;
pub use menu::{
    ContextMenuArea, MenuBar, MenuChosen, MenuItem, MenuList, context_request, open_context_menu,
};
pub use multiline::MultilineEditor;
pub use nav::{Breadcrumb, BreadcrumbChosen, StatusBar, StatusItem, Toolbar};
pub use node_canvas::{
    CanvasComment, CanvasEdit, CanvasModel, CanvasNode, CanvasPin, CanvasSelection, CanvasStats,
    CanvasView, CanvasWire, ClipOp, ClipboardRequested, CommentEdited, CommentRequested,
    ConnectCheck, ConnectRefused, ConnectRequested, DETAIL_ZOOM, DeleteRequested,
    DisconnectRequested, HEADER_H, LABEL_ZOOM, MINIMAP, NODE_MIN_W, NodeCanvas, NodeChosen,
    NodesMoved, PIN_ROW_H, PinRef, REROUTE, RerouteRequested, SearchItems, ZOOM_MAX, ZOOM_MIN,
};
pub use numeric::{NumericCommitted, NumericField};
pub use palette::{
    AssetEntry, AssetIndex, AssetRefChanged, AssetRefPicker, CommandInvoked, CommandPaletteButton,
    MemoryAssetIndex, PickItem, PickList, open_command_palette, open_command_palette_ui,
};
pub use progress::{Progress, ProgressBar};
pub use property_grid::{PropKind, Property, PropertyGrid, PropertyGridParts, property_grid};
pub use range::RangeSlider;
pub use relay::{SignalChanged, SignalRelay};
pub use rich_text::{LinkActivated, RichText, Span, SpanKind};
pub use scroll::{
    Axis, ScrollArea, ScrollParts, Scrollbar, SplitHandle, SplitParts, scroll_area, splitter,
};
pub use search::{PathChosen, PathField, SEARCH_DEBOUNCE, SearchChanged, SearchField};
pub use slider::{Slider, SliderEdit, SliderPhase};
pub use spinner::Spinner;
pub use table::{
    Column, ColumnResized, HEADER_KEY_BASE, SortDir, TableModel, TableSorted, VirtualTable,
};
pub use tabs::{TabClosed, TabMoved, TabSelected, Tabs};
pub use text_field::{Submitted, TextField};
pub use toggle::{Checkbox, RadioGroup};
pub use vector::{
    FramePosEditor, FramePosParts, GIMBAL_MARGIN_DEG, QuatEditor, QuatParts, Rebaser, VectorEditor,
    VectorParts, euler_to_quat, frame_pos_editor, quat_editor, quat_to_euler, vector_editor,
};

pub(crate) use button::activation;

use crate::style::Theme;
use crate::text::TextStyle;

/// Body text from the type scale.
pub(crate) fn body(theme: &Theme) -> TextStyle {
    TextStyle::body(theme.type_scale.body)
}

/// Small text from the type scale.
pub(crate) fn small(theme: &Theme) -> TextStyle {
    TextStyle::body(theme.type_scale.small)
}

/// Vertical padding of controls.
pub(crate) fn control_pad_y(theme: &Theme) -> f32 {
    theme.space[1] + 2.0
}

/// Horizontal padding of controls.
pub(crate) fn control_pad_x(theme: &Theme) -> f32 {
    theme.space[3]
}

/// The minimum height of a control (a comfortable pointer target).
pub(crate) const CONTROL_MIN_H: f32 = 28.0;

/// Where composite widgets (colour picker, vector editors, property grid) are built:
/// the [`Ui`](crate::Ui) at build time, or an [`EventCx`](crate::widget::EventCx) when a
/// popover builds its content as it opens. Either way the parts join the retained tree
/// like any other widget.
pub trait Build {
    fn add_boxed_widget(
        &mut self,
        parent: crate::WidgetId,
        key: crate::Key,
        style: crate::NodeStyle,
        widget: Box<dyn crate::Widget>,
    ) -> Result<crate::WidgetId, crate::UiError>;
    fn runtime(&mut self) -> &mut crate::Runtime;
    fn theme_ref(&self) -> &Theme;
    /// Show or hide a part (a warning that only appears when needed).
    fn hide(&mut self, id: crate::WidgetId, hidden: bool);
    /// A `Send` handle for posting work from other threads into this window (async
    /// thumbnails); `None` where there is no window queue.
    fn poster(&self) -> Option<crate::ui::Poster> {
        None
    }
}

impl dyn Build + '_ {
    /// Add a widget.
    pub fn add<W: crate::Widget>(
        &mut self,
        parent: crate::WidgetId,
        key: impl Into<crate::Key>,
        style: crate::NodeStyle,
        widget: W,
    ) -> Result<crate::WidgetId, crate::UiError> {
        self.add_boxed_widget(parent, key.into(), style, Box::new(widget))
    }
    /// Create a signal.
    pub fn signal<T: PartialEq + 'static>(&mut self, v: T) -> crate::Signal<T> {
        self.runtime().signal(v)
    }
}

impl Build for crate::Ui {
    fn add_boxed_widget(
        &mut self,
        parent: crate::WidgetId,
        key: crate::Key,
        style: crate::NodeStyle,
        widget: Box<dyn crate::Widget>,
    ) -> Result<crate::WidgetId, crate::UiError> {
        self.add_boxed(parent, key, style, widget)
    }
    fn runtime(&mut self) -> &mut crate::Runtime {
        self.rt_mut()
    }
    fn theme_ref(&self) -> &Theme {
        self.theme()
    }
    fn hide(&mut self, id: crate::WidgetId, hidden: bool) {
        let _ = self.set_hidden(id, hidden);
    }
    fn poster(&self) -> Option<crate::ui::Poster> {
        Some(crate::Ui::poster(self))
    }
}

impl Build for crate::widget::EventCx<'_> {
    fn add_boxed_widget(
        &mut self,
        parent: crate::WidgetId,
        key: crate::Key,
        style: crate::NodeStyle,
        widget: Box<dyn crate::Widget>,
    ) -> Result<crate::WidgetId, crate::UiError> {
        self.ui.add_boxed(parent, key, style, widget)
    }
    fn runtime(&mut self) -> &mut crate::Runtime {
        self.rt_mut()
    }
    fn theme_ref(&self) -> &Theme {
        self.theme()
    }
    fn hide(&mut self, id: crate::WidgetId, hidden: bool) {
        self.set_hidden(id, hidden);
    }
    fn poster(&self) -> Option<crate::ui::Poster> {
        Some(self.ui.poster())
    }
}
