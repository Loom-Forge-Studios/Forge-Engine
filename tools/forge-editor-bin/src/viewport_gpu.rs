//! The viewport's `forge-render` host (Ch.21 §21.13, §21.21 "Viewport"; DoD M2-32).
//!
//! The editor library never touches a GPU: a viewport cell **requests** a frame (its camera,
//! size, drawables, selection) in the surface registry whenever what it shows changed, and
//! paints a `Primitive::Viewport` for its surface. The winit runner calls
//! [`ViewportRenderHost::render_dirty`] once per window frame that has damage, just before
//! the UI is drawn, on the UI's own device; the host renders each dirty request with a
//! `forge_render::Renderer` into a texture and the runner registers it for compositing
//! under the viewport's overlay lines.
//!
//! **Only through `forge-render`'s public API** (the WP-U6 coordination rule): `Renderer`,
//! `Scene`, `Instance`, `Material`, `MeshData`, `Camera`. The camera is the editor camera
//! as-is — `f64` `FramePos` and orientation — and `forge-render`'s last mile does the
//! camera-relative `f32` narrowing, so the rendered image and the editor's overlay lines are
//! the same projection.
//!
//! **A plugin world's frames.** A cell showing a plugin's world ([`SurfaceFrame::world`])
//! is drawn by the host's [`WorldRenderer`] for that world (a program adds them with
//! [`ViewportRenderHost::add_world_renderer`]; the base editor has none): it gets the cell's
//! target — its `Renderer`, its texture and a slot for its own per-target state — and may
//! ask for the same frame again when it paced its work over frames.
//!
//! An idle viewport requests nothing, so the host renders nothing (D-5).

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use forge_editor::services::EditorServices;
use forge_editor::viewport::surface::{SurfaceFrame, ViewportSurfaces, WorldContent};
use forge_frames::{DQuat, DVec3, FramePos, FrameResolver};
use forge_gpu::{GpuDevice, wgpu};
use forge_render::{
    Camera, DirectionalLight, Instance, Material, MaterialId, MeshData, MeshId, RenderOptions,
    Renderer, Scene,
};
use forge_trace::Tracer;

/// The base colour of an entity's cube, and of a selected one, and of the ground.
pub const ENTITY_COLOUR: [f64; 4] = [0.80, 0.32, 0.18, 1.0];
pub const SELECTED_COLOUR: [f64; 4] = [0.95, 0.72, 0.10, 1.0];
pub const GROUND_COLOUR: [f64; 4] = [0.30, 0.31, 0.33, 1.0];

/// One viewport cell's render target, as a [`WorldRenderer`] gets it.
pub struct WorldTarget<'a> {
    /// The cell's renderer (its meshes, materials and passes; kept while the cell's size is).
    pub renderer: &'a mut Renderer,
    /// The texture the cell composites.
    pub texture: &'a wgpu::Texture,
    /// The world renderer's own state for this target (empty on a new target).
    pub state: &'a mut Option<Box<dyn Any>>,
}

/// Draws a plugin world's frames ([`SurfaceFrame::world`]) for the host (see the module
/// docs).
pub trait WorldRenderer {
    /// The concrete renderer (a program's own code and tests read it).
    fn as_any(&self) -> &dyn Any;
    /// The concrete renderer, mutably.
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Whether this renderer draws `content`.
    fn draws(&self, content: &dyn WorldContent) -> bool;
    /// Draw frame `f` (whose world content is `content`) into `target`. `Ok(true)`: it has
    /// more to do for the same frame (its work paced over frames): the host renders it again
    /// and wakes the viewports.
    fn render(
        &mut self,
        dev: &GpuDevice,
        target: WorldTarget<'_>,
        f: &SurfaceFrame,
        content: &dyn WorldContent,
    ) -> Result<bool, String>;
}

struct Target {
    renderer: Renderer,
    texture: wgpu::Texture,
    /// The texture's view, made with it: handed out every frame (a handle clone, no new view).
    view: wgpu::TextureView,
    size: (u32, u32),
    cube: MeshId,
    ground: MeshId,
    plain: MaterialId,
    selected: MaterialId,
    earth: MaterialId,
    /// A world renderer's state for this target ([`WorldTarget::state`]).
    world: Option<Box<dyn Any>>,
}

/// See the module docs.
pub struct ViewportRenderHost {
    surfaces: Rc<RefCell<ViewportSurfaces>>,
    frames: Arc<dyn FrameResolver>,
    /// Where render times and the adapter lane are reported (the profiler reads it).
    tracer: &'static Tracer,
    targets: BTreeMap<u32, Target>,
    /// The renderers of plugin worlds' frames.
    worlds: Vec<Box<dyn WorldRenderer>>,
    /// Frames rendered (tests read it).
    pub rendered: u64,
}

impl ViewportRenderHost {
    /// Attach a host to the editor's viewports; `name` is what their stats overlay shows.
    pub fn new(services: &EditorServices, name: &str) -> Self {
        services.viewport_surfaces.borrow_mut().attach_host(name);
        Self {
            surfaces: Rc::clone(&services.viewport_surfaces),
            frames: Arc::clone(&services.frames),
            tracer: services.tracer,
            targets: BTreeMap::new(),
            rendered: 0,
            worlds: Vec::new(),
        }
    }

    /// Draw a plugin world's frames with `r` (the first renderer that draws a frame's
    /// content draws it).
    pub fn add_world_renderer(&mut self, r: Box<dyn WorldRenderer>) {
        self.worlds.push(r);
    }

    /// The world renderer of type `T`, if one was added (tests read its reports).
    #[must_use]
    pub fn world_renderer<T: 'static>(&self) -> Option<&T> {
        self.worlds
            .iter()
            .find_map(|r| r.as_any().downcast_ref::<T>())
    }

    /// The world renderer of type `T`, mutably (a test attaches its probes).
    pub fn world_renderer_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.worlds
            .iter_mut()
            .find_map(|r| r.as_any_mut().downcast_mut::<T>())
    }

    /// The texture last rendered for surface `id` (tests read it back).
    pub fn texture(&self, id: u32) -> Option<&wgpu::Texture> {
        self.targets.get(&id).map(|t| &t.texture)
    }

    /// Take every viewport's pending frame request without rendering it (each is marked
    /// presented): their surface ids and sizes in physical pixels. Play-in-editor of the 2D
    /// sample draws those viewports itself while its level runs.
    pub fn take_dirty_sizes(&mut self) -> Vec<(u32, (u32, u32))> {
        let dirty = self.surfaces.borrow_mut().take_dirty();
        let mut out = Vec::with_capacity(dirty.len());
        for (id, frame) in dirty {
            self.surfaces.borrow_mut().presented(id, Ok(()));
            out.push((id, frame.size_px));
        }
        out
    }

    /// Render every viewport that asked for a frame, on `dev`. Returns the textures to
    /// register, by surface id.
    pub fn render_dirty(&mut self, dev: &GpuDevice) -> Vec<(u32, wgpu::TextureView)> {
        let dirty = self.surfaces.borrow_mut().take_dirty();
        let mut out = Vec::with_capacity(dirty.len());
        for (id, frame) in dirty {
            let t0 = std::time::Instant::now();
            let r = {
                let _z = self.tracer.zone("viewport.render_cpu");
                self.render_one(dev, id, &frame)
            };
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok((view, more)) => {
                    self.rendered += 1;
                    if more {
                        // A world renderer paced its work: the same frame again.
                        self.surfaces.borrow_mut().again(id);
                    }
                    self.tracer
                        .set_lane(&dev.label(), vec![(0.0, ms, format!("viewport {id:#x}"))]);
                    // One profiler frame per rendered view (the host is the producer; the
                    // profiler's own redraws never mark a frame, so it cannot wake itself).
                    self.tracer.frame_mark();
                    out.push((id, view));
                    self.surfaces.borrow_mut().presented(id, Ok(()));
                }
                Err(e) => self.surfaces.borrow_mut().presented(id, Err(e)),
            }
        }
        out
    }

    fn target<'a>(
        targets: &'a mut BTreeMap<u32, Target>,
        dev: &GpuDevice,
        id: u32,
        size: (u32, u32),
    ) -> Result<&'a mut Target, String> {
        let stale = targets.get(&id).is_none_or(|t| t.size != size);
        if stale {
            targets.remove(&id);
            let opts = RenderOptions::new(size.0, size.1);
            let mut renderer = Renderer::new(dev, opts).map_err(|e| e.to_string())?;
            let cube = renderer
                .add_mesh(&MeshData::cube())
                .map_err(|e| e.to_string())?;
            let ground = renderer
                .add_mesh(&MeshData::quad())
                .map_err(|e| e.to_string())?;
            let mat = |c: [f64; 4], rough: f64| Material {
                base_color: c,
                roughness: rough,
                ..Material::default()
            };
            let plain = renderer.add_material(mat(ENTITY_COLOUR, 0.55));
            let selected = renderer.add_material(mat(SELECTED_COLOUR, 0.4));
            let earth = renderer.add_material(mat(GROUND_COLOUR, 0.9));
            let texture = dev.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("forge-editor viewport"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            targets.insert(
                id,
                Target {
                    renderer,
                    texture,
                    view,
                    size,
                    cube,
                    ground,
                    plain,
                    selected,
                    earth,
                    world: None,
                },
            );
        }
        targets
            .get_mut(&id)
            .ok_or_else(|| "viewport target missing".to_string())
    }

    fn render_one(
        &mut self,
        dev: &GpuDevice,
        id: u32,
        f: &SurfaceFrame,
    ) -> Result<(wgpu::TextureView, bool), String> {
        let size = (f.size_px.0.clamp(1, 8192), f.size_px.1.clamp(1, 8192));
        if let Some(w) = f.world.clone() {
            let content = w.0.as_ref();
            let k = self
                .worlds
                .iter()
                .position(|r| r.draws(content))
                .ok_or_else(|| "no renderer for this viewport world".to_string())?;
            let t = Self::target(&mut self.targets, dev, id, size)?;
            let more = self.worlds[k].render(
                dev,
                WorldTarget {
                    renderer: &mut t.renderer,
                    texture: &t.texture,
                    state: &mut t.world,
                },
                f,
                content,
            )?;
            let view = t.view.clone();
            return Ok((view, more));
        }
        let frames = Arc::clone(&self.frames);
        let t = Self::target(&mut self.targets, dev, id, size)?;
        let mut instances: Vec<Instance> = f
            .drawables
            .iter()
            .filter(|d| !d.layer)
            .map(|d| {
                let mat = if f.selection.contains(&d.key) {
                    t.selected
                } else {
                    t.plain
                };
                Instance::new(d.pos, t.cube, mat)
                    .oriented(d.rotation)
                    .scaled(d.scale)
            })
            .collect();
        // A ground plane under the camera on its frame's y = 0 (the grid's plane).
        let cam = f.camera;
        let reach = (cam.pos.local.y.abs() * 40.0).max(2000.0);
        instances.push(
            Instance::new(
                FramePos::new(
                    cam.pos.frame,
                    DVec3::new(cam.pos.local.x, -0.002, cam.pos.local.z),
                ),
                t.ground,
                t.earth,
            )
            .oriented(DQuat::from_axis_angle(
                DVec3::X,
                -core::f64::consts::FRAC_PI_2,
            ))
            .scaled3(DVec3::new(reach, reach, 1.0)),
        );
        let scene = Scene {
            instances,
            sun: Some(DirectionalLight {
                frame: cam.pos.frame,
                toward_light: DVec3::new(0.35, 0.85, 0.4),
                color: [1.0, 0.97, 0.92],
                illuminance: 100_000.0,
                casts_shadows: true,
            }),
            ambient: [3500.0, 3900.0, 4600.0],
            ev100: 15.0,
            background: [5200.0, 6800.0, 9400.0],
            ..Scene::default()
        };
        let camera = Camera {
            pos: cam.pos,
            orientation: cam.orientation,
            fov_y: cam.fov_y,
        };
        t.renderer
            .render_scene(dev, &scene, &camera, frames.as_ref(), f.tick, &t.texture)
            .map_err(|e| e.to_string())?;
        Ok((t.view.clone(), false))
    }
}
