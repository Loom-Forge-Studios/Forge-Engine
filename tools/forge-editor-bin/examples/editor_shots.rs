//! Render the editor shell to PNG, headless (a design-review aid; nothing gates on it):
//! the main window with the undo history, notifications, settings and keybindings panels
//! docked, after a few edits by the user and by an automation session.
//!
//! ```text
//! cargo run -p forge-editor-bin --example editor_shots -- <out-dir>
//! ```

use std::path::PathBuf;
use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{Shell, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_panels_core::{PanelsCore, panel_ids};
use forge_ui::dock::{Axis, DockNode, Layout};
use forge_ui::geom::PhysicalSize;
use forge_ui::render::TargetId;
use forge_ui::render_wgpu::{GpuContext, WgpuRenderer};
use forge_ui::{Size, Ui, UiConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "editor_shots".into()),
    );
    std::fs::create_dir_all(&out)?;
    let core_panels = PanelsCore::new()?;
    let stand_in = StandInPanels::without(&panel_ids())?;
    let cfg = assemble(builtin_preset("3d")?, &[&core_panels, &stand_in], &[], None)?;
    let core = EditorCore::new();
    let client = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    let mut shell = Shell::new(cfg, Box::new(client))?;
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
                0.2,
                DockNode::tabs(&["forge.hierarchy", "forge.keybindings"]),
            ),
            (0.5, DockNode::tabs(&["forge.settings", "forge.viewport"])),
            (
                0.3,
                DockNode::split(
                    Axis::Vertical,
                    vec![
                        (0.55, DockNode::tabs(&["forge.undo_history"])),
                        (0.45, DockNode::tabs(&["forge.notifications"])),
                    ],
                ),
            ),
        ],
    )));
    let em = shell.emitter().clone();
    em.emit(EditorCommand::Spawn {
        name: "Cube".into(),
        parent: None,
    });
    em.emit(EditorCommand::SetSetting {
        key: "editor.snap_translate".into(),
        value: Some(Value::Bool(true)),
    });
    let mut auto = EditorCore::connect(
        &core,
        Issuer::Automation {
            session: "s7".into(),
            tool: "apply".into(),
        },
    );
    auto.apply(
        EditorCommand::SetSetting {
            key: "editor.grid_size".into(),
            value: Some(Value::Float(0.5)),
        },
        None,
    );
    em.emit(EditorCommand::Rename {
        entity: forge_cmd::EntityKey(999),
        name: "x".into(),
    });
    let ctx = GpuContext::headless(false).or_else(|_| GpuContext::headless(true))?;
    let mut r = WgpuRenderer::from_context(&ctx)?;
    let t = TargetId(1);
    r.add_offscreen_target(
        t,
        PhysicalSize {
            w: size.w as u32,
            h: size.h as u32,
        },
    );
    for i in 1..6u64 {
        ui.frame(Duration::from_millis(16 * i));
        let acts = ui.take_actions();
        shell.on_actions(&mut ui, acts);
        shell.update_window(&mut ui);
    }
    ui.frame(Duration::from_millis(200));
    ui.damage_all();
    ui.render(&mut r, t)?;
    let (w, h, px) = r.read_pixels(t)?;
    let file = out.join("editor_main.png");
    let f = std::fs::File::create(&file)?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&px)?;
    println!("{}", file.display());
    Ok(())
}
