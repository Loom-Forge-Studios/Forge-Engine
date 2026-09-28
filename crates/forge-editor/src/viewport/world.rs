//! Worlds a plugin shows in the viewport (Ch.21 §21.21 "Viewport"): a place of the plugin's
//! own — its own frames, its own content, its own camera moves — that the viewport's cells
//! show **instead of the project scene** while it is shown.
//!
//! The viewport owns the cells (their cameras and controllers, their render surfaces, the
//! overlay's tools and the input). A [`ViewportWorld`] is asked, through this trait only:
//!
//! * whether it is shown, and in which frames and at which moment its cameras are
//!   ([`ViewportWorld::frames`], [`ViewportWorld::tick`]);
//! * to take a cell's camera to generated content the session asked the camera to focus on
//!   ([`ViewportWorld::focus`], the navigator's "Fly to" and any other session camera
//!   focus), now or once its background work has planned it ([`WorldLaunch::Pending`], then
//!   [`ViewportWorld::poll`] when its [`ViewportWorld::feed`] ticks);
//! * to animate a cell's camera one display frame at a time ([`ViewportWorld::step`]) and to
//!   stop when the user takes the camera or presses Esc ([`ViewportWorld::cancel`]);
//! * to paint a cell ([`ViewportWorld::paint`]: its overlay lines and text, and the frame it
//!   requests from a GPU host through [`crate::viewport::surface::SurfaceFrame::world`]).
//!
//! A world comes from a plugin through the `ViewportWorld` extension point
//! ([`ViewportWorldPoint`]): the viewport panel makes one from the first factory registered.
//! With none (the base editor), the viewport shows the project scene only. Nothing here is
//! project state: showing a world, and moving a camera in it, are session state (I7).

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use forge_frames::{FrameResolver, Tick};
use forge_plugin::ExtensionPoint;
use forge_ui::LiveCell;
use forge_ui::widget::PaintCx;

use crate::services::EditorServices;
use crate::viewport::camera::EditorCamera;

/// The cells' cameras, as the viewport lends them to a world.
pub trait CellCameras {
    /// How many cells there are (shown or not).
    fn count(&self) -> usize;
    /// Cell `cell`'s camera.
    fn camera(&self, cell: usize) -> Option<EditorCamera>;
    /// Put cell `cell`'s camera at `camera` (its controller follows).
    fn set_camera(&mut self, cell: usize, camera: EditorCamera);
}

/// One cell, as a world paints it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldCell {
    /// Which cell.
    pub index: usize,
    /// Its camera now.
    pub camera: EditorCamera,
    /// Its render surface (the external-texture id a GPU host renders into).
    pub surface: u32,
}

/// How a [`ViewportWorld::focus`] request went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldLaunch {
    /// The camera started moving now (animate the cell).
    Started,
    /// Planned in the background: [`ViewportWorld::poll`] starts it when the world's feed
    /// ticks.
    Pending,
    /// The world has no such place (the viewport then frames an entity of the project with
    /// that seed path, if one exists).
    Nothing,
}

/// What one animation frame of a world's camera move did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorldStep {
    /// The move arrived (or there is none): no more frames needed for it.
    pub done: bool,
    /// The move holds where it is until the world's background work is ready (no frames
    /// until [`ViewportWorld::poll`] resumes it).
    pub waiting: bool,
    /// Keep asking for frames anyway (a world's W2 idle control only).
    pub keep_animating: bool,
}

/// A world a plugin shows in the viewport (see the module docs).
pub trait ViewportWorld {
    /// The concrete world, for the plugin's own code and its tests.
    fn as_any(&self) -> &dyn Any;
    /// Whether the cells show this world (else the project scene).
    fn shown(&self) -> bool;
    /// The frames the cells' cameras are in while [`ViewportWorld::shown`].
    fn frames(&self) -> Option<Arc<dyn FrameResolver>>;
    /// The moment the cells show while shown.
    fn tick(&self) -> Tick;
    /// The session asked the camera to focus on generated content at `seed_path` (named
    /// `name`): take cell `cell`'s camera there.
    fn focus(
        &mut self,
        cams: &mut dyn CellCameras,
        cell: usize,
        seed_path: &str,
        name: &str,
    ) -> WorldLaunch;
    /// The world's feed ticked (its background work finished something): a cell whose
    /// camera starts (or resumes) moving now, if any.
    fn poll(&mut self, cams: &mut dyn CellCameras) -> Option<usize>;
    /// Whether the world is moving cell `cell`'s camera.
    fn moving(&self, cell: usize) -> bool;
    /// Whether a move for cell `cell` is being planned in the background.
    fn pending(&self, cell: usize) -> bool;
    /// One display frame of the camera move, `dt` seconds after the last.
    fn step(&mut self, cams: &mut dyn CellCameras, dt: f64) -> WorldStep;
    /// The user took the camera or pressed Esc: stop the move (and any planning) where it
    /// is. Whether something stopped.
    fn cancel(&mut self) -> bool;
    /// Back to the project scene: stop everything and show nothing.
    fn leave(&mut self);
    /// Cell `cell` is hidden: let go of what it held.
    fn cell_hidden(&mut self, cell: usize);
    /// Paint `cell` (only while shown): the cell's rect is `cx.rect()`, already filled.
    fn paint(&self, cx: &mut PaintCx, cell: &WorldCell);
    /// Bumped when the world's background work finished something (the viewport wakes on
    /// it; an idle world never bumps it).
    fn feed(&self) -> Arc<LiveCell>;
    /// Whether background work is still running for what is shown.
    fn busy(&self) -> bool;
}

/// Makes a panel's world over the editor's services.
pub type ViewportWorldFactory =
    Arc<dyn Fn(&Rc<EditorServices>) -> Box<dyn ViewportWorld> + Send + Sync>;

/// The `ViewportWorld` extension point (`forge.editor.viewport_world`): a plugin provides the
/// world the viewport shows its generated places in. The viewport panel makes its world
/// from the first factory registered; with none it shows the project scene only.
pub struct ViewportWorldPoint;

impl ExtensionPoint for ViewportWorldPoint {
    type Item = ViewportWorldFactory;
    const ID: &'static str = "forge.editor.viewport_world";
    const NAME: &'static str = "ViewportWorld";
}
