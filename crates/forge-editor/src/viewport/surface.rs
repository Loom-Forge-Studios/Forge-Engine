//! Attaching a GPU scene renderer to viewports (Ch.21 §21.13 "viewport composites").
//!
//! The editor library never touches a GPU. A viewport **requests** a frame here — its
//! camera, size, drawables and selection — whenever what it shows changed, and paints a
//! `Primitive::Viewport` for its surface id; the renderer host (the editor binary's
//! `forge-render` host, run by the winit runner just before the UI frame is drawn) takes the
//! dirty requests, renders each into a texture and registers it under that id, and the UI
//! composites it under the viewport's overlay lines. With no host attached (headless, no
//! adapter) the viewport draws its placeholder: the grid, axes and bounding boxes alone.
//!
//! A viewport that did not change requests nothing, so an idle editor renders nothing
//! (D-5): the host only ever works for a request.

use std::collections::BTreeMap;
use std::rc::Rc;

use forge_cmd::EntityKey;
use forge_frames::Tick;

use crate::viewport::camera::EditorCamera;
use crate::viewport::scene::Drawable;

/// The first external-texture id viewports use (the UI's own textures never reach here).
pub const SURFACE_BASE: u32 = 0x5650_0000;

/// What a viewport wants rendered.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceFrame {
    pub camera: EditorCamera,
    /// Physical pixels.
    pub size_px: (u32, u32),
    pub drawables: Rc<Vec<Drawable>>,
    pub selection: Vec<EntityKey>,
    pub tick: Tick,
    /// A plugin world's frame ([`crate::viewport::world::ViewportWorld`]): the camera is in
    /// that world's frames (not the project scene's) and the host's renderer for the world
    /// draws its content. `None`: the project scene.
    pub world: Option<WorldFrame>,
}

/// What a plugin world hands the renderer host for one frame. Opaque to the editor: the
/// host's renderer for that world knows its type ([`WorldContent::as_any`]).
pub trait WorldContent {
    /// The concrete content, for the host's renderer.
    fn as_any(&self) -> &dyn std::any::Any;
    /// Whether `other` is the same content (an unchanged request is not rendered again).
    fn same(&self, other: &dyn WorldContent) -> bool;
}

/// A plugin world's part of a [`SurfaceFrame`].
#[derive(Clone)]
pub struct WorldFrame(pub Rc<dyn WorldContent>);

impl std::fmt::Debug for WorldFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorldFrame")
    }
}

impl PartialEq for WorldFrame {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.0, &o.0) || self.0.same(o.0.as_ref())
    }
}

#[derive(Debug, Default)]
struct Slot {
    frame: Option<SurfaceFrame>,
    dirty: bool,
    presented: u64,
}

/// The viewports' render surfaces (see the module docs). Shared (`Rc<RefCell<_>>`) by the
/// viewports and the host, both on the UI thread.
#[derive(Default)]
pub struct ViewportSurfaces {
    next: u32,
    host: Option<String>,
    slots: BTreeMap<u32, Slot>,
    error: Option<String>,
    requests: u64,
    /// Wakes the viewports (their panel's live feed) when the host has more to do for a
    /// frame it could not finish in one go (a world's uploads paced over frames).
    waker: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for ViewportSurfaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewportSurfaces")
            .field("host", &self.host)
            .field("slots", &self.slots.len())
            .field("requests", &self.requests)
            .finish_non_exhaustive()
    }
}

impl ViewportSurfaces {
    pub fn new() -> Self {
        Self::default()
    }

    /// A renderer host attached (its name, shown in the viewport's stats overlay).
    pub fn attach_host(&mut self, name: &str) {
        self.host = Some(name.to_string());
    }
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// A new surface id (an external-texture id).
    pub fn allocate(&mut self) -> u32 {
        let id = SURFACE_BASE + self.next;
        self.next += 1;
        self.slots.insert(id, Slot::default());
        id
    }

    /// The viewport went away.
    pub fn release(&mut self, id: u32) {
        self.slots.remove(&id);
    }

    /// Ask for `frame` to be rendered into surface `id`. An unchanged request is not
    /// re-queued.
    pub fn request(&mut self, id: u32, frame: SurfaceFrame) {
        if let Some(s) = self.slots.get_mut(&id)
            && s.frame.as_ref() != Some(&frame)
        {
            s.frame = Some(frame);
            s.dirty = true;
            self.requests += 1;
        }
    }

    /// The viewports' waker (see [`ViewportSurfaces::again`]).
    pub fn set_waker(&mut self, waker: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.waker = Some(waker);
    }

    /// The host rendered surface `id` but has more to do for the same frame (uploads it
    /// paced): render it again, and wake the viewports so a window frame comes.
    pub fn again(&mut self, id: u32) {
        if let Some(s) = self.slots.get_mut(&id)
            && s.frame.is_some()
        {
            s.dirty = true;
            if let Some(w) = &self.waker {
                w();
            }
        }
    }

    /// Requests made so far (a steady viewport makes none).
    pub fn requests(&self) -> u64 {
        self.requests
    }

    /// The host takes every dirty request.
    pub fn take_dirty(&mut self) -> Vec<(u32, SurfaceFrame)> {
        self.slots
            .iter_mut()
            .filter(|(_, s)| s.dirty)
            .filter_map(|(id, s)| {
                s.dirty = false;
                s.frame.clone().map(|f| (*id, f))
            })
            .collect()
    }

    /// The host rendered surface `id` (or failed, with the reason the viewport shows).
    pub fn presented(&mut self, id: u32, result: Result<(), String>) {
        if let Some(s) = self.slots.get_mut(&id) {
            s.presented += 1;
        }
        self.error = result.err();
    }

    /// Frames the host has rendered into surface `id`.
    pub fn presented_count(&self, id: u32) -> u64 {
        self.slots.get(&id).map_or(0, |s| s.presented)
    }

    /// The host's last error, if its last frame failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}
