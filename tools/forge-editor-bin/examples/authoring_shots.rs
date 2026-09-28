//! Render the authoring editors to PNG, headless (a design-review aid for WP-U11; nothing
//! gates on it): the sequencer with a keyed clip, the animation state machine with a blend
//! space and transitions, and the localisation editor with two locales.
//!
//! ```text
//! cargo run -p forge-editor-bin --example authoring_shots -- <out-dir>
//! ```

use std::path::PathBuf;
use std::time::Duration;

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::authoring::anim::{self as an, AnimDoc, BlendKind, ParamKind};
use forge_editor::authoring::strings::{self as ls, StringsDoc};
use forge_editor::authoring::timeline::{self as tl, TimelineDoc};
use forge_editor::core::EditorCore;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{Shell, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_panels_authoring::PanelsAuthoring;
use forge_panels_core::PanelsCore;
use forge_ui::dock::{DockNode, Layout};
use forge_ui::geom::PhysicalSize;
use forge_ui::render::TargetId;
use forge_ui::render_wgpu::{GpuContext, WgpuRenderer};
use forge_ui::{Size, Ui, UiConfig};

type R = Result<(), Box<dyn std::error::Error>>;

fn pump(shell: &mut Shell, ui: &mut Ui, t: &mut u64) {
    for _ in 0..6 {
        *t += 16;
        ui.frame(Duration::from_millis(*t));
        let acts = ui.take_actions();
        shell.on_actions(ui, acts);
        shell.update_window(ui);
    }
}

fn main() -> R {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "authoring_shots".into()),
    );
    std::fs::create_dir_all(&out)?;
    let core_panels = PanelsCore::new()?;
    let authoring = PanelsAuthoring::new()?;
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_authoring::panel_ids());
    let stand_in = StandInPanels::without(&real)?;
    let cfg = assemble(
        builtin_preset("3d")?,
        &[&core_panels, &authoring, &stand_in],
        &[],
        None,
    )?;
    let core = EditorCore::new();
    let key = EditorCore::read(&core, |p| p.next_key());
    let client = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    let mut shell = Shell::new(cfg, Box::new(client))?;
    let size = Size::new(1440.0, 900.0);
    let mut ui = Ui::new(UiConfig {
        size,
        ..UiConfig::default()
    })?;
    shell.build_main(&mut ui)?;
    let mut t = 0u64;
    let em = shell.emitter().clone();
    em.emit(EditorCommand::Spawn {
        name: "Crate".into(),
        parent: None,
    });
    let e: EntityKey = key;
    for (p, v) in [
        ("transform.position.local", Value::Vec3([0.0, 0.0, 0.0])),
        ("light.intensity", Value::Float(1.0)),
    ] {
        em.emit(EditorCommand::SetProperty {
            entity: e,
            path: p.into(),
            value: v,
        });
    }
    pump(&mut shell, &mut ui, &mut t);
    shell.session_mut().set_selection(vec![e]);
    // A clip with two property tracks and an event track.
    let (clip, cmds) = tl::new_clip(&TimelineDoc::read(&shell.mirror()), "Walk cycle");
    em.emit_all("clip", cmds);
    pump(&mut shell, &mut ui, &mut t);
    for path in ["light.intensity", "transform.position.local"] {
        let doc = TimelineDoc::read(&shell.mirror());
        if let Some(c) = doc.clips.get(&clip)
            && let Ok((_, cmds)) = tl::add_property_track(&shell.mirror(), c, e, path, 0.0)
        {
            em.emit_all("track", cmds);
        }
        pump(&mut shell, &mut ui, &mut t);
    }
    for (tid, keys) in [
        (
            "crate_light_intensity",
            vec![
                (1.0, Value::Float(4.0)),
                (2.5, Value::Float(0.5)),
                (4.0, Value::Float(3.0)),
            ],
        ),
        (
            "crate_transform_position_local",
            vec![
                (1.5, Value::Vec3([2.0, 1.0, 0.0])),
                (3.0, Value::Vec3([4.0, 0.0, 1.0])),
            ],
        ),
    ] {
        for (at, v) in keys {
            let doc = TimelineDoc::read(&shell.mirror());
            if let Some(c) = doc.clips.get(&clip)
                && let Some(tr) = c.tracks.get(tid)
                && let Ok(cmds) = tl::set_key(c, tr, at, v)
            {
                em.emit_all("key", cmds);
            }
            pump(&mut shell, &mut ui, &mut t);
        }
    }
    {
        let doc = TimelineDoc::read(&shell.mirror());
        if let Some(c) = doc.clips.get(&clip) {
            let (id, cmds) = tl::add_event_track(c, "Footsteps");
            em.emit_all("events", cmds);
            pump(&mut shell, &mut ui, &mut t);
            let doc = TimelineDoc::read(&shell.mirror());
            if let Some(c) = doc.clips.get(&clip)
                && let Some(tr) = c.tracks.get(&id)
            {
                for at in [0.5, 1.5, 2.5, 3.5] {
                    let doc = TimelineDoc::read(&shell.mirror());
                    let (Some(c), Some(tr)) = (
                        doc.clips.get(&clip),
                        doc.clips.get(&clip).and_then(|c| c.tracks.get(&tr.id)),
                    ) else {
                        continue;
                    };
                    if let Ok(cmds) = tl::set_key(c, tr, at, Value::Text("step".into())) {
                        em.emit_all("event", cmds);
                    }
                    pump(&mut shell, &mut ui, &mut t);
                }
            }
        }
    }
    // A state machine: parameters, a 1D locomotion blend, jump states and transitions.
    let (m, cmds) = an::new_machine(
        &AnimDoc::read(&shell.mirror()),
        "Locomotion",
        "Humanoid",
        "Idle",
    );
    em.emit_all("machine", cmds);
    pump(&mut shell, &mut ui, &mut t);
    for (p, k) in [
        ("speed", ParamKind::Float),
        ("grounded", ParamKind::Bool),
        ("jump", ParamKind::Trigger),
    ] {
        let doc = AnimDoc::read(&shell.mirror());
        if let Some(mc) = doc.machines.get(&m) {
            em.emit_all("param", an::add_param(mc, p, k).1);
        }
        pump(&mut shell, &mut ui, &mut t);
    }
    let doc = AnimDoc::read(&shell.mirror());
    if let Some(mc) = doc.machines.get(&m)
        && let Ok((_, cmds)) = an::add_blend_state(
            mc,
            "Move",
            BlendKind::OneD,
            "speed",
            None,
            &[("Idle", 0.0, 0.0), ("Walk", 1.0, 0.0), ("Run", 3.0, 0.0)],
            (240.0, 0.0),
        )
    {
        em.emit_all("blend", cmds);
    }
    pump(&mut shell, &mut ui, &mut t);
    for (name, motion, x, y) in [
        ("Jump", "Jump", 240.0, 200.0),
        ("Fall", "Fall", 480.0, 200.0),
    ] {
        let doc = AnimDoc::read(&shell.mirror());
        if let Some(mc) = doc.machines.get(&m) {
            em.emit_all("state", an::add_state(mc, name, motion, x, y).1);
        }
        pump(&mut shell, &mut ui, &mut t);
    }
    for (from, to, conds) in [
        ("idle", "move", vec!["speed > 0.1"]),
        ("move", "idle", vec!["speed < 0.1"]),
        ("any", "jump", vec!["jump"]),
        ("jump", "fall", vec!["!grounded"]),
        ("fall", "idle", vec!["grounded"]),
    ] {
        let doc = AnimDoc::read(&shell.mirror());
        if let Some(mc) = doc.machines.get(&m)
            && let Ok((_, cmds)) = an::add_transition(mc, from, to, &conds, 0.2)
        {
            em.emit_all("transition", cmds);
        }
        pump(&mut shell, &mut ui, &mut t);
    }
    // Strings: two locales, a few keys, a missing translation and a placeholder slip.
    for (tag, name) in [("fr-FR", "Fran\u{e7}ais"), ("de-DE", "Deutsch")] {
        let doc = StringsDoc::read(&shell.mirror());
        if let Ok((_, cmds)) = ls::add_locale(&doc, tag, name) {
            em.emit_all("locale", cmds);
        }
        pump(&mut shell, &mut ui, &mut t);
    }
    for (k, en, fr, de) in [
        ("menu.play", "Play", Some("Jouer"), Some("Spielen")),
        ("menu.settings", "Settings", Some("Param\u{e8}tres"), None),
        ("menu.quit", "Quit", Some("Quitter"), Some("Beenden")),
        (
            "hud.score",
            "Score: {score}",
            Some("Score : {points}"),
            Some("Punkte: {score}"),
        ),
    ] {
        let doc = StringsDoc::read(&shell.mirror());
        if let Ok(cmds) = ls::add_string(&doc, k, en) {
            em.emit_all("string", cmds);
        }
        pump(&mut shell, &mut ui, &mut t);
        for (loc, text) in [("fr_fr", fr), ("de_de", de)] {
            let doc = StringsDoc::read(&shell.mirror());
            if let (Some(text), Ok((tb, key))) = (text, ls::split_key(k))
                && let Some(en) = doc.entries.get(&(tb, key))
                && let Ok(cmds) = ls::set_text(&doc, en, loc, text)
            {
                em.emit_all("translation", cmds);
            }
            pump(&mut shell, &mut ui, &mut t);
        }
    }
    let ctx = GpuContext::headless(false).or_else(|_| GpuContext::headless(true))?;
    let mut r = WgpuRenderer::from_context(&ctx)?;
    let target = TargetId(1);
    r.add_offscreen_target(
        target,
        PhysicalSize {
            w: size.w as u32,
            h: size.h as u32,
        },
    );
    for panel in ["forge.sequencer", "forge.anim_graph", "forge.localisation"] {
        shell
            .dock_mut()
            .set_layout(Layout::new(DockNode::tabs(&[panel])));
        pump(&mut shell, &mut ui, &mut t);
        pump(&mut shell, &mut ui, &mut t);
        t += 200;
        ui.frame(Duration::from_millis(t));
        ui.damage_all();
        ui.render(&mut r, target)?;
        let (w, h, px) = r.read_pixels(target)?;
        let file = out.join(format!("{}.png", panel.trim_start_matches("forge.")));
        let f = std::fs::File::create(&file)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&px)?;
        println!("{}", file.display());
    }
    Ok(())
}
