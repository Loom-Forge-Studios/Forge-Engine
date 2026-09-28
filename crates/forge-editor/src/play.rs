//! Play-in-editor, the UI side of M2-5 (Ch.21 §21.21 "Play controls", Ch.37 §37.5).
//!
//! **Play runs against your sandbox.** Pressing Play forks a *simulation world* from the
//! edit world (the project as the mirror shows it); the simulation steps that copy, the
//! viewport shows it, and Stop throws it away. Nothing a play session does writes to the
//! project — the edit world is untouched by construction, because the backend is handed a
//! snapshot and never a way to send commands (`test_play_controls`: the project's state
//! hash is identical before Play and after Stop).
//!
//! **Play controls are commands on the bus — session commands, not project edits.** Play,
//! pause, step and stop are the `forge.play.control` command ([`PLAY_CMD`], built by
//! [`play_command`]), an `Invoke` the core registers ([`handler`]) exactly as it registers
//! the brushes, so the play-controls panel, the menu and chords, a script, an automation session
//! and an remote client all issue the same command through their own [`crate::client::BusClient`].
//! Its planner validates the arguments and changes nothing: the project is untouched by
//! construction, the command never enters the undo history (undoing "Play" would mean nothing), and
//! the bus audits it with its issuer. The core hands every applied session command to the clients
//! that follow them ([`crate::core::SessionCommand`], in stream order); the shell's play core
//! applies it as a [`PlayCommand`] to the [`PlayBackend`], which records it in its session log with
//! the tick it took effect at — the recording WP-13's deterministic replay consumes.
//!
//! **The backend.** [`PlayBackend`] is the trait the shell drives; the editor's backend is
//! WP-13's play core, [`crate::sim_bridge::SimPlay`] over `forge_sim::PlaySession`: Play forks
//! a simulation world (`forge_sim::SimWorld`, an ECS world on `forge_core`'s regionized
//! scheduler) from the mirror, steps it at a fixed 60 Hz, and records every control, input and
//! checkpoint hash to a replay file that re-runs bit-for-bit — the same core `forge
//! --headless` runs on the same `forge.play.control` commands (`test_headless_parity`).
//! [`PlayCommand`], [`PlayState`] and [`PlayLogEntry`] are `forge_sim`'s: one set of play types
//! for the panel, the shell, headless and the replay file.

use std::collections::BTreeMap;
use std::sync::Arc;

use forge_cmd::EntityKey;
use forge_frames::{TICKS_PER_SECOND, Tick};
use forge_ui::LiveCell;

use crate::mirror::ProjectMirror;

pub use forge_sim::edit::{P_ACCELERATION, P_SPIN, P_VELOCITY};
pub use forge_sim::{
    PlayCommand, PlayLogEntry, PlayState, SimTransform, step_of_tick, tick_of_step,
};

/// The nominal length of one fixed 60 Hz step, in canonical ticks. Step `n` is at
/// [`tick_of_step`]`(n)` = `floor(n * 10^6 / 60)`, so consecutive steps are 16 666 or 16 667
/// ticks apart and 60 steps are exactly one second; use [`ticks_after`] for exact arithmetic.
pub const SIM_DT: i64 = TICKS_PER_SECOND / 60;

/// The tick `n` fixed steps after the step at tick `t`.
#[must_use]
pub fn ticks_after(t: Tick, n: u32) -> Tick {
    tick_of_step(step_of_tick(t).saturating_add(u64::from(n)))
}

/// The play-control command (`Invoke` target): `{"control": "play" | "pause" | "step" |
/// "stop", "ticks": n}` (`ticks` for a step only, default 1).
pub const PLAY_CMD: &str = "forge.play.control";
/// Ticks one step command may advance at most (ten minutes of simulation).
pub const MAX_STEP_TICKS: u32 = 36_000;

/// The bus command for a play control.
pub fn play_command(cmd: PlayCommand) -> forge_cmd::EditorCommand {
    let args = match cmd {
        PlayCommand::Play => serde_json::json!({ "control": "play" }),
        PlayCommand::Pause => serde_json::json!({ "control": "pause" }),
        PlayCommand::Step(n) => serde_json::json!({ "control": "step", "ticks": n }),
        PlayCommand::Stop => serde_json::json!({ "control": "stop" }),
    };
    forge_cmd::EditorCommand::Invoke {
        target: PLAY_CMD.into(),
        args: args.to_string(),
    }
}

/// Read a play command's arguments (the planner's check and the play core's decoding are
/// this one function, so what the bus accepts is exactly what the core runs).
pub fn parse_play_args(args: &serde_json::Value) -> Result<PlayCommand, String> {
    let control = args
        .get("control")
        .and_then(serde_json::Value::as_str)
        .ok_or("\"control\" must be \"play\", \"pause\", \"step\" or \"stop\"")?;
    let ticks = args.get("ticks");
    if ticks.is_some() && control != "step" {
        return Err(format!("\"ticks\" belongs to a step, not to {control:?}"));
    }
    Ok(match control {
        "play" => PlayCommand::Play,
        "pause" => PlayCommand::Pause,
        "stop" => PlayCommand::Stop,
        "step" => {
            let n = match ticks {
                None => 1,
                Some(t) => t
                    .as_u64()
                    .filter(|n| (1..=u64::from(MAX_STEP_TICKS)).contains(n))
                    .ok_or(format!("\"ticks\" must be 1..={MAX_STEP_TICKS}"))?,
            };
            PlayCommand::Step(n as u32)
        }
        other => {
            return Err(format!(
                "unknown control {other:?}: use \"play\", \"pause\", \"step\" or \"stop\""
            ));
        }
    })
}

/// Decode a session command the core handed over, if it is a play control.
pub fn decode(target: &str, args: &str) -> Option<Result<PlayCommand, String>> {
    (target == PLAY_CMD).then(|| {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| e.to_string())
            .and_then(|v| parse_play_args(&v))
    })
}

fn plan_play(
    _b: &mut forge_cmd::DiffBuilder<'_>,
    args: &serde_json::Value,
) -> Result<(), forge_cmd::CmdError> {
    // A session command: validated, and no change to the project, ever.
    parse_play_args(args)
        .map(drop)
        .map_err(|why| forge_cmd::CmdError::BadArgs {
            target: PLAY_CMD.into(),
            why,
        })
}

/// Not an undo step, not redoable, open to every issuer (an automation session may play the
/// sandbox).
pub const PLAY_POLICY: forge_cmd::CommandPolicy = forge_cmd::CommandPolicy {
    human_only: false,
    undoable: false,
    redoable: false,
};

/// The play command's planner, for the editor core to register on its bus.
pub fn handler() -> (
    &'static str,
    forge_cmd::CommandPolicy,
    crate::project::Planner,
) {
    (PLAY_CMD, PLAY_POLICY, plan_play)
}

// ---- simulation inputs ---------------------------------------------------------------------

/// The simulation-input command (`Invoke` target, WP-19): its arguments are one
/// `forge_sim::SimInput` in its JSON form, `{"entity": 3, "action": {"Impulse": [0, 5, 0]}}`.
/// A session command like [`PLAY_CMD`]: the bus validates, audits and delivers it in stream
/// order to the client hosting the play core (the shell, `forge --headless`), which queues it
/// for the next step boundary; it never changes the project and never enters the undo
/// history. Every client — a game's input layer in the editor, a script, an automation session, a
/// headless `input` line — sends the same command, so a recorded input is the same input
/// whichever client produced it (gate `C-sim-input-command`).
pub const PLAY_INPUT_CMD: &str = "forge.play.input";

/// The bus command for a simulation input.
pub fn input_command(i: forge_sim::SimInput) -> forge_cmd::EditorCommand {
    forge_cmd::EditorCommand::Invoke {
        target: PLAY_INPUT_CMD.into(),
        args: serde_json::to_string(&i).unwrap_or_default(),
    }
}

/// Read a simulation input's arguments: well-formed and finite (the planner's check and the
/// play core's decoding are this one function). Whether the entity exists in the running
/// simulation is the play core's check when the input arrives (`SIM-0002`/`SIM-0004`).
pub fn parse_input_args(args: &serde_json::Value) -> Result<forge_sim::SimInput, String> {
    let i: forge_sim::SimInput = serde_json::from_value(args.clone()).map_err(|e| {
        format!("not a simulation input ({e}): {{\"entity\": <key>, \"action\": {{\"Impulse\": [x, y, z]}}}}")
    })?;
    let finite = match i.action {
        forge_sim::InputAction::SetVelocity(v)
        | forge_sim::InputAction::Impulse(v)
        | forge_sim::InputAction::SetAcceleration(v) => v.iter().all(|x| x.is_finite()),
        forge_sim::InputAction::SetSpin(s) => s.is_finite(),
    };
    if finite {
        Ok(i)
    } else {
        Err("a simulation input must be finite".into())
    }
}

/// Decode a session command the core handed over, if it is a simulation input.
pub fn decode_input(target: &str, args: &str) -> Option<Result<forge_sim::SimInput, String>> {
    (target == PLAY_INPUT_CMD).then(|| {
        serde_json::from_str::<serde_json::Value>(args)
            .map_err(|e| e.to_string())
            .and_then(|v| parse_input_args(&v))
    })
}

fn plan_input(
    _b: &mut forge_cmd::DiffBuilder<'_>,
    args: &serde_json::Value,
) -> Result<(), forge_cmd::CmdError> {
    // A session command: validated, and no change to the project, ever.
    parse_input_args(args)
        .map(drop)
        .map_err(|why| forge_cmd::CmdError::BadArgs {
            target: PLAY_INPUT_CMD.into(),
            why,
        })
}

/// The simulation-input command's planner (the same policy as the play controls: not an
/// undo step, open to every issuer), for the editor core to register on its bus.
pub fn input_handler() -> (
    &'static str,
    forge_cmd::CommandPolicy,
    crate::project::Planner,
) {
    (PLAY_INPUT_CMD, PLAY_POLICY, plan_input)
}

/// The play core, as the play controls and the viewport reach it (see the module docs).
pub trait PlayBackend {
    /// What this backend is (the play controls show it).
    fn backend(&self) -> &str;
    fn state(&self) -> PlayState;
    /// Apply a control. `edit` is the edit world to fork from (read-only: the backend gets
    /// no way to change the project).
    fn control(&mut self, cmd: PlayCommand, edit: &ProjectMirror);
    /// Run a playing simulation up to `now` (the editor's clock) in fixed ticks. Returns the
    /// ticks run.
    fn advance(&mut self, now: std::time::Duration) -> u32;
    /// The simulation's current tick (0 when stopped).
    fn tick(&self) -> Tick;
    /// Simulated transforms of the entities the simulation moved.
    fn transforms(&self) -> &BTreeMap<EntityKey, SimTransform>;
    /// Bumped whenever the simulation changed (state or transforms).
    fn revision(&self) -> u64;
    /// The session's controls so far (cleared by the next Play from stopped).
    fn log(&self) -> &[PlayLogEntry];
    /// The live feed the profiler follows (bumped per simulated step batch).
    fn feed(&self) -> Arc<LiveCell>;
    /// Wall-clock milliseconds the last advance spent (the profiler's `sim.step` row).
    fn last_step_ms(&self) -> f64;
    /// The last control or advance the backend could not carry out (a fork refused, a step
    /// that failed), taken once: `control` and `advance` return nothing, so the shell asks
    /// here and shows it in the console rather than dropping it.
    fn take_error(&mut self) -> Option<String> {
        None
    }
    /// Queue a simulation input (a [`PLAY_INPUT_CMD`] session command the bus delivered) for
    /// the next step boundary. `Err` says why it was refused (no simulation, an unknown
    /// entity); a backend without inputs refuses every one.
    fn input(&mut self, i: forge_sim::SimInput) -> Result<(), String> {
        let _ = i;
        Err(format!(
            "{}: this play backend takes no simulation inputs",
            self.backend()
        ))
    }
}

/// The frame interval the shell wakes at while a simulation plays (60 Hz).
pub const PLAY_FRAME: std::time::Duration = std::time::Duration::from_micros(16_667);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_after_follows_the_step_schedule() {
        assert_eq!(ticks_after(Tick(0), 1), Tick(16_666));
        assert_eq!(ticks_after(Tick(16_666), 1), Tick(33_333));
        assert_eq!(ticks_after(Tick(33_333), 3), Tick(83_333));
        assert_eq!(ticks_after(Tick(0), 60), Tick(TICKS_PER_SECOND));
    }

    #[test]
    fn a_play_command_round_trips_through_the_bus_form() {
        for cmd in [
            PlayCommand::Play,
            PlayCommand::Pause,
            PlayCommand::Step(7),
            PlayCommand::Stop,
        ] {
            let forge_cmd::EditorCommand::Invoke { target, args } = play_command(cmd) else {
                panic!("not an Invoke");
            };
            assert_eq!(decode(&target, &args), Some(Ok(cmd)));
        }
        assert!(decode(PLAY_CMD, "{\"control\":\"step\",\"ticks\":0}").is_some_and(|r| r.is_err()));
        assert_eq!(decode("forge.other", "{}"), None);
    }

    #[test]
    fn an_input_command_round_trips_and_refuses_what_is_not_an_input() {
        let i = forge_sim::SimInput {
            entity: EntityKey(3),
            action: forge_sim::InputAction::Impulse([0.0, 5.0, 0.0]),
        };
        let forge_cmd::EditorCommand::Invoke { target, args } = input_command(i) else {
            panic!("not an Invoke");
        };
        assert_eq!(target, PLAY_INPUT_CMD);
        assert_eq!(decode_input(&target, &args), Some(Ok(i)));
        assert_eq!(decode_input(PLAY_CMD, &args), None);
        for bad in [
            "{}",
            "{\"entity\":3}",
            "{\"entity\":3,\"action\":{\"Teleport\":[0,0,0]}}",
            "not json",
        ] {
            assert!(
                decode_input(PLAY_INPUT_CMD, bad).is_some_and(|r| r.is_err()),
                "{bad}"
            );
        }
        let nan = forge_sim::SimInput {
            entity: EntityKey(3),
            action: forge_sim::InputAction::SetSpin(f64::NAN),
        };
        assert!(parse_input_args(&serde_json::json!(nan)).is_err());
    }
}
