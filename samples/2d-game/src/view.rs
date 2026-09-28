//! The game view's GPU host (feature `app`): renders the level with `forge-2d`'s
//! `Renderer2d` into a texture the size of the view, on the UI's own device, for the
//! window's runner to composite under the menus (`UiApp::render_external`). The binary's
//! window and its headless modes (`--shot`, `--probe`) both draw through it.

use forge_2d::Error2d;
use forge_2d::atlas::Atlas;
use forge_2d::render::{RenderOptions2d, Renderer2d};
use forge_2d::scenes::SceneTextures;
use forge_gpu::{GpuDevice, wgpu};

use crate::Game;

struct Target {
    renderer: Renderer2d,
    textures: SceneTextures,
    atlas: Atlas,
    texture: wgpu::Texture,
    size: (u32, u32),
}

/// See the module docs.
#[derive(Default)]
pub struct GameViewHost {
    target: Option<Target>,
    /// Frames rendered (the binary's report and tests read it).
    pub rendered: u64,
}

impl GameViewHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Render `game` at `size` pixels on `dev`; the texture's view, to register. The
    /// renderer and its uploaded textures are kept and rebuilt only when the size changes.
    pub fn render(
        &mut self,
        dev: &GpuDevice,
        game: &Game,
        size: (u32, u32),
    ) -> Result<wgpu::TextureView, Error2d> {
        let size = (size.0.max(1), size.1.max(1));
        let t = match self.target.take() {
            Some(t) if t.size == size => self.target.insert(t),
            _ => self.target.insert(Self::build(dev, size)?),
        };
        let frame = game.frame(&t.textures, &t.atlas)?;
        t.renderer.render(dev, &frame, &t.texture)?;
        self.rendered += 1;
        Ok(t.texture
            .create_view(&wgpu::TextureViewDescriptor::default()))
    }

    /// The texture last rendered (headless modes read it back).
    #[must_use]
    pub fn texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.texture)
    }

    fn build(dev: &GpuDevice, size: (u32, u32)) -> Result<Target, Error2d> {
        let mut renderer = Renderer2d::new(dev, RenderOptions2d::new(size.0, size.1))?;
        let (textures, atlas) = forge_2d::scenes::upload(&mut renderer, dev)?;
        let texture = dev.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("forge-2d-game view"),
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
        Ok(Target {
            renderer,
            textures,
            atlas,
            texture,
            size,
        })
    }
}
