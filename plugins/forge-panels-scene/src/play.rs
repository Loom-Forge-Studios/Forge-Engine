//! **Play controls** (`forge.play_controls`, Ch.21 §21.21, DoD M2-33): play, pause, step
//! and stop, issued as the `forge.play.control` command on the bus (`forge_editor::play`) —
//! the same command an automation session, a script or a remote client sends; the shell's play core
//! runs it when the stream delivers it.
//!
//! Play-in-editor runs against **your own sandbox** (Ch.37 §37.5): Play forks a simulation
//! from the edit world, the viewport shows the simulation, and Stop throws it away — the
//! project is never written (`test_play_controls` checks its state hash). The panel shows
//! the state, the simulation time, the session's control log (what WP-13's replay records)
//! and which backend runs it: WP-13's play core (`forge_editor::sim_bridge::SimPlay`).

use std::cell::Cell;
use std::rc::Rc;

use forge_editor::panels::PanelCx;
use forge_editor::play::{PlayCommand, PlayState, play_command};
use forge_frames::TICKS_PER_SECOND;
use forge_ui::widgets::{Button, Container, Label, LabelKind, Pressed};
use forge_ui::{NodeStyle, Role};

/// The panel's text for a state.
pub fn state_text(state: PlayState, tick: forge_frames::Tick) -> String {
    let secs = tick.0 as f64 / TICKS_PER_SECOND as f64;
    match state {
        PlayState::Stopped => forge_ui::tr!("Stopped \u{2014} editing the project").to_string(),
        PlayState::Playing => forge_ui::trf!(
            "\u{25b6} Playing \u{00b7} {secs} s (tick {tick})",
            secs = format!("{secs:.3}"),
            tick = tick.0
        ),
        PlayState::Paused => forge_ui::trf!(
            "\u{23f8} Paused \u{00b7} {secs} s (tick {tick})",
            secs = format!("{secs:.3}"),
            tick = tick.0
        ),
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the play controls and the session state"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let services = pb.services();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Play controls")),
        )?;
        let play = pb.b.add(
            bar,
            "play",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("\u{25b6} Play")).primary(),
        )?;
        let pause = pb.b.add(
            bar,
            "pause",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("\u{23f8} Pause")),
        )?;
        let step = pb.b.add(
            bar,
            "step",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("\u{23ed} Step")),
        )?;
        let stop = pb.b.add(
            bar,
            "stop",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("\u{23f9} Stop")),
        )?;
        let (st0, t0, b0) = {
            let p = services.play.borrow();
            (
                p.state(),
                p.tick(),
                forge_ui::l10n::tr_str(p.backend()).into_owned(),
            )
        };
        let state = pb.b.signal(state_text(st0, t0));
        let log = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "state",
            NodeStyle::leaf().padding(space[1]),
            Label::new(state).kind(LabelKind::Heading),
        )?;
        pb.b.add(
            pb.parent,
            "sandbox",
            NodeStyle::leaf().padding(space[1]),
            Label::new(forge_ui::tr!(
                "Play runs in your own sandbox: the simulation is forked from the project and \
                 discarded on Stop. Edits you make while playing still go to the project."
            ))
            .kind(LabelKind::Muted)
            .wrapping(),
        )?;
        pb.b.add(
            pb.parent,
            "log",
            NodeStyle::leaf().padding(space[1]),
            Label::new(log).kind(LabelKind::Small).wrapping(),
        )?;
        pb.b.add(
            pb.parent,
            "backend",
            NodeStyle::leaf().padding(space[1]),
            Label::new(b0).kind(LabelKind::Small).wrapping(),
        )?;
        for (id, which) in [(play, 0u8), (pause, 1), (step, 2), (stop, 3)] {
            let s = Rc::clone(&services);
            pb.on(id, move |act, _: &Pressed| {
                // The state decides what a button means; the control itself is a command on
                // the bus (`forge.play.control`), exactly what an automation session sends — the
                // play core runs it when the stream delivers it.
                let state = s.play.borrow().state();
                let cmd = match (which, state) {
                    (0, _) => PlayCommand::Play,
                    (1, PlayState::Paused) => PlayCommand::Play,
                    (1, _) => PlayCommand::Pause,
                    // Step pauses a running simulation first (the play core does it).
                    (2, _) => PlayCommand::Step(1),
                    _ => PlayCommand::Stop,
                };
                if cmd == PlayCommand::Stop && s.faults.play_writes_project() {
                    // W2 positive control only: "keep the simulation's results".
                    let cmds: Vec<forge_cmd::EditorCommand> = s
                        .play
                        .borrow()
                        .transforms()
                        .iter()
                        .map(|(k, (pos, _, _))| forge_cmd::EditorCommand::SetProperty {
                            entity: *k,
                            path: "transform.position.local".into(),
                            value: forge_cmd::Value::Vec3([pos.local.x, pos.local.y, pos.local.z]),
                        })
                        .collect();
                    act.cmd.emit_all(forge_ui::tr!("Keep simulation"), cmds);
                }
                act.cmd.emit(play_command(cmd));
            });
        }
        let seen = Rc::new(Cell::new(u64::MAX));
        pb.sync(bar, move |sy| {
            let p = services.play.borrow();
            if p.revision() == seen.get() {
                return Ok(());
            }
            seen.set(p.revision());
            state.set(sy.ui.rt_mut(), state_text(p.state(), p.tick()));
            let entries: Vec<String> = p
                .log()
                .iter()
                .rev()
                .take(6)
                .map(|e| {
                    forge_ui::trf!(
                        "{command} at tick {tick}",
                        command = format!("{:?}", e.command),
                        tick = e.tick.0
                    )
                })
                .collect();
            log.set(
                sy.ui.rt_mut(),
                if entries.is_empty() {
                    String::new()
                } else {
                    forge_ui::trf!(
                        "Session log (newest first): {items}",
                        items = entries.join(" \u{00b7} ")
                    )
                },
            );
            Ok(())
        });
        Ok(())
    });
}
