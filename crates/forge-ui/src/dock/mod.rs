//! Docking (Ch.21 §21.17): tabs, splits, floating windows on any monitor, drag-to-dock with
//! drop previews, maximise — and the layout as plain RON data.
//!
//! * [`model`] — the [`Layout`] data and every edit as a pure, normalising operation.
//! * [`geometry`] — where groups, strips and handles sit; drop targets; window placement.
//! * [`area`] — the [`DockArea`] widget over one tree, its tab strips, splitters and preview.
//! * [`windows`] — the [`DockController`] that keeps one layout across OS windows.
//!
//! Docking lives in `forge-ui` rather than the editor because it is a widget-layer concern
//! a game's tools UI may use too; which panels exist, their titles and content come from
//! the application through [`PanelHost`] (in the editor: the `EditorPanel` extension
//! point, I16).

pub mod area;
pub mod geometry;
pub mod model;
pub mod windows;

pub use area::{
    DockArea, DockChanged, DockFaults, DockOp, DockRequest, DockSplitter, DockTabStrip, DockTree,
    DropPreview, FLOAT_SIZE, PanelHost, apply_local, dock_area, panel_frame_id,
};
pub use geometry::{DropHit, Frac, Geometry, MonitorInfo, drop_at, geometry, place_window};
pub use model::{
    AreaId, Axis, DockNode, DropTarget, DropZone, FloatingWindow, LAYOUT_VERSION, Layout, PanelId,
    Side,
};
pub use windows::{DockController, HostFactory};
