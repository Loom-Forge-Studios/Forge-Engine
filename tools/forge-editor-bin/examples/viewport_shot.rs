//! Render the editor with the viewport, inspector, play controls and profiler to PNG,
//! headless, with the viewport's `forge-render` host attached (a design-review aid; nothing
//! gates on it): a few crates, one selected with the move gizmo.
//!
//! ```text
//! cargo run -p forge-editor-bin --example viewport_shot -- <out-dir>
//! ```

use std::path::PathBuf;
use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{Shell, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor_bin::viewport_gpu::ViewportRenderHost;
use forge_panels_core::PanelsCore;
use forge_panels_scene::PanelsScene;
use forge_ui::dock::{Axis, DockNode, Layout};
use forge_ui::geom::PhysicalSize;
use forge_ui::render::{ExternalTexture, TargetId};
use forge_ui::render_wgpu::{GpuContext, WgpuRenderer};
use forge_ui::{Size, Ui, UiConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "viewport_shots".into()),
    );
    std::fs::create_dir_all(&out)?;
    let core_panels = PanelsCore::new()?;
    let scene_panels = PanelsScene::new()?;
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_scene::panel_ids());
    let stand_in = StandInPanels::without(&real)?;
    let cfg = assemble(
        builtin_preset("3d")?,
        &[&core_panels, &scene_panels, &stand_in],
        &[],
        None,
    )?;
    let core = EditorCore::new();
    let client = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    let mut shell = Shell::new(cfg, Box::new(client))?;
    let mut host = ViewportRenderHost::new(&shell.handles().services_rc(), "forge-render (shot)");
    let size = Size::new(1440.0, 900.0);
    let mut ui = Ui::new(UiConfig {
        size,
        ..UiConfig::default()
    })?;
    shell.build_main(&mut ui)?;
    shell.dock_mut().set_layout(Layout::new(DockNode::split(
        Axis::Horizontal,
        vec![
            (
                0.72,
                DockNode::split(
                    Axis::Vertical,
                    vec![
                        (0.72, DockNode::tabs(&["forge.viewport"])),
                        (
                            0.28,
                            DockNode::tabs(&["forge.profiler", "forge.play_controls"]),
                        ),
                    ],
                ),
            ),
            (0.28, DockNode::tabs(&["forge.inspector"])),
        ],
    )));
    // A few crates, through a script client (the same bus as the user).
    let mut script = EditorCore::connect(
        &core,
        Issuer::Script {
            path: "shot.fscript".into(),
        },
    );
    let mut keys = Vec::new();
    for (i, x) in [-2.5f64, 0.0, 2.5].iter().enumerate() {
        let e = EditorCore::read(&core, |p| p.next_key());
        keys.push(e);
        script.apply(
            EditorCommand::Spawn {
                name: format!("Crate {i}"),
                parent: None,
            },
            None,
        );
        for (path, v) in [
            ("transform.position.frame", Value::Int(0)),
            ("transform.position.local", Value::Vec3([*x, 0.5, 0.0])),
            ("transform.yaw", Value::Float(20.0 * i as f64)),
            ("transform.scale", Value::Float(1.0)),
        ] {
            script.apply(
                EditorCommand::SetProperty {
                    entity: e,
                    path: path.into(),
                    value: v,
                },
                None,
            );
        }
    }
    let _ = script.pump();
    shell.session_mut().set_selection(vec![keys[1]]);

    let ctx = GpuContext::headless(false).or_else(|_| GpuContext::headless(true))?;
    let dev = ctx.pool.devices().get(ctx.index).ok_or("no pool device")?;
    let mut r = WgpuRenderer::from_context(&ctx)?;
    let t = TargetId(1);
    r.add_offscreen_target(
        t,
        PhysicalSize {
            w: size.w as u32,
            h: size.h as u32,
        },
    );
    for i in 1..8u64 {
        ui.frame(Duration::from_millis(16 * i));
        let acts = ui.take_actions();
        shell.on_actions(&mut ui, acts);
        shell.update_window(&mut ui);
    }
    ui.frame(Duration::from_millis(200));
    for (id, view) in host.render_dirty(dev) {
        r.register_external(ExternalTexture(id), view);
    }
    ui.damage_all();
    ui.render(&mut r, t)?;
    let (w, h, px) = r.read_pixels(t)?;
    let file = out.join("editor_viewport.png");
    let f = std::fs::File::create(&file)?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&px)?;
    println!(
        "{} ({} viewport frames rendered)",
        file.display(),
        host.rendered
    );
    Ok(())
}
