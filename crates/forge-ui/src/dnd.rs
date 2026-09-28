//! Typed drag-and-drop (Ch.21 §21.9).
//!
//! Drags carry typed in-process payloads. A drop target declares what it accepts by
//! answering `DragOver` with a [`DropVerdict`]; the preview shows "accepted",
//! "refused (reason)" or an insertion line. OS file drops arrive as `Files`.
//! **Dragging out to the OS is not supported** — `winit` has no API for it (§21.25).

use std::path::PathBuf;

/// What a drag carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragPayload {
    Entities(Vec<u64>),
    Asset(String),
    Panel(String),
    GraphNodes(Vec<u64>),
    Files(Vec<PathBuf>),
}

impl DragPayload {
    pub fn kind(&self) -> PayloadKind {
        match self {
            DragPayload::Entities(_) => PayloadKind::Entities,
            DragPayload::Asset(_) => PayloadKind::Asset,
            DragPayload::Panel(_) => PayloadKind::Panel,
            DragPayload::GraphNodes(_) => PayloadKind::GraphNodes,
            DragPayload::Files(_) => PayloadKind::Files,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PayloadKind {
    Entities,
    Asset,
    Panel,
    GraphNodes,
    Files,
}

/// A drop target's answer while a drag hovers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropVerdict {
    Accepted,
    /// Refused, with the reason shown in the preview.
    Refused(String),
    /// Accepted as an insertion before child `index` (lists, trees, tab strips).
    Insert {
        index: usize,
    },
}

/// The in-flight drag session the `Ui` tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct DragSession {
    pub payload: DragPayload,
    pub source: crate::id::WidgetId,
    /// The widget under the pointer and its verdict.
    pub over: Option<(crate::id::WidgetId, DropVerdict)>,
}
