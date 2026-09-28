//! Layers a plugin keeps over the project scene in the viewport (Ch.21 §21.21 "Viewport"):
//! content the plugin derives from the project itself — a field its commands edit, say — that
//! every cell draws **on top of** the scene, and that the plugin's own tools read.
//!
//! A [`ViewportLayer`] is asked, through this trait only:
//!
//! * to follow the project ([`ViewportLayer::sync`], once per viewport sync, with the mirror
//!   every client sees: a layer is derived state, never project state, I7);
//! * to draw itself into one cell ([`ViewportLayer::draw`]: camera-relative lines in `f64`,
//!   through the same [`LineSink`] the grid and the gizmos use).
//!
//! A plugin's tool reaches its layer through [`ToolView::layers`] by type
//! ([`ViewportLayers::get`]); the plugin's panels reach it through
//! [`crate::services::EditorServices::viewport_layers`]. An entity a layer shows by itself
//! carries [`P_LAYER`]: the scene draws no box for it and a click does not pick it.
//!
//! Layers come from plugins through the `ViewportLayer` extension point
//! ([`ViewportLayerPoint`]); `shell::assemble` makes one of each registered factory, in
//! registry order. With none (the base editor), the viewport draws the scene only.

use std::any::Any;
use std::sync::Arc;

use forge_plugin::ExtensionPoint;

use crate::mirror::ProjectMirror;
use crate::viewport::scene::LineSink;
use crate::viewport::tool::ToolView;

/// The entity property that says a viewport layer shows the entity (its value: the layer's
/// name). The scene draws no bounding box for it and picking skips it.
pub const P_LAYER: &str = "viewport.layer";

/// Content a plugin keeps over the scene (see the module docs).
pub trait ViewportLayer: Any {
    /// The concrete layer, for the plugin's tools, panels and tests.
    fn as_any(&self) -> &dyn Any;
    /// The concrete layer, mutably (a panel changes its session settings).
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Follow the project: whether anything this layer draws changed (the viewport then
    /// repaints). Called on every viewport sync; a layer that has nothing to do returns
    /// `false` without walking the project.
    fn sync(&mut self, mirror: &ProjectMirror) -> bool;
    /// Draw into one cell (`view`: the cell's camera, size and frames).
    fn draw(&self, view: &ToolView<'_>, sink: &mut LineSink<'_>);
}

/// Makes one layer (per editor).
pub type ViewportLayerFactory = Arc<dyn Fn() -> Box<dyn ViewportLayer> + Send + Sync>;

/// The `ViewportLayer` extension point (`forge.editor.viewport_layer`).
pub struct ViewportLayerPoint;

impl ExtensionPoint for ViewportLayerPoint {
    type Item = ViewportLayerFactory;
    const ID: &'static str = "forge.editor.viewport_layer";
    const NAME: &'static str = "ViewportLayer";
}

/// The layers every viewport of one editor shares, in registry order.
#[derive(Default)]
pub struct ViewportLayers {
    items: Vec<(String, Box<dyn ViewportLayer>)>,
}

impl std::fmt::Debug for ViewportLayers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.items.iter().map(|(k, _)| k))
            .finish()
    }
}

impl ViewportLayers {
    /// No layers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One layer of each factory in `reg`, in registry order.
    #[must_use]
    pub fn from_registry(reg: &forge_plugin::Registry<ViewportLayerPoint>) -> Self {
        Self {
            items: reg.iter().map(|(k, f)| (k.to_string(), f())).collect(),
        }
    }

    /// Add `layer` as `key` (replacing one of the same key, which is returned).
    pub fn insert(
        &mut self,
        key: &str,
        layer: Box<dyn ViewportLayer>,
    ) -> Option<Box<dyn ViewportLayer>> {
        match self.items.iter_mut().find(|(k, _)| k == key) {
            Some((_, l)) => Some(std::mem::replace(l, layer)),
            None => {
                self.items.push((key.to_string(), layer));
                None
            }
        }
    }

    /// How many layers there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The first layer of type `T`.
    #[must_use]
    pub fn get<T: ViewportLayer>(&self) -> Option<&T> {
        self.items
            .iter()
            .find_map(|(_, l)| l.as_any().downcast_ref::<T>())
    }

    /// The first layer of type `T`, mutably.
    pub fn get_mut<T: ViewportLayer>(&mut self) -> Option<&mut T> {
        self.items
            .iter_mut()
            .find_map(|(_, l)| l.as_any_mut().downcast_mut::<T>())
    }

    /// Follow the project with every layer; whether any changed.
    pub fn sync(&mut self, mirror: &ProjectMirror) -> bool {
        let mut changed = false;
        for (_, l) in &mut self.items {
            changed |= l.sync(mirror);
        }
        changed
    }

    /// Draw every layer into one cell, in order.
    pub fn draw(&self, view: &ToolView<'_>, sink: &mut LineSink<'_>) {
        for (_, l) in &self.items {
            l.draw(view, sink);
        }
    }
}
