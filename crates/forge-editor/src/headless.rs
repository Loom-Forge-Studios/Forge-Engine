//! Headless mode (Ch.34 §34.4: headless is the same binary): the core with no UI at all,
//! driven by a script — one request per line. `forge --headless` and
//! `forge-editor --headless` both run this.
//!
//! ```text
//! {"Spawn":{"name":"Cube","parent":null}}      an EditorCommand as JSON (its wire form)
//! {"Invoke":{"target":"forge.play.control","args":"{\"control\":\"play\"}"}}
//!                                              a play control, as the GUI and automation sessions send it
//! undo                                         Ctrl+Z
//! redo                                         Ctrl+Y
//! play | pause | stop                          sugar for that forge.play.control command
//! step N                                       sugar: {"control":"step","ticks":N}
//! run N                                        let N fixed steps of a playing simulation pass
//! input {"entity":3,"action":{"Impulse":[0,5,0]}}   a simulation input (forge_sim::SimInput)
//! sim-hash                                     print the simulation's state hash
//! # a comment, or a blank line
//! ```
//!
//! **One route for every request.** Editor requests go through a [`BusClient`] issued as
//! `Script { path }` — the headless runner is just another client of the core, like the GUI
//! and like an automation session. **Play controls are the bus's `forge.play.control` session
//! command** (`crate::play`), whichever way the line spells it: the runner follows session commands
//! and runs each one the bus delivers on the play core, [`SimPlay`] — the type the editor's shell
//! runs them on — forked from the core's project (the GUI forks from its mirror; the two must
//! agree, `test_headless_parity`). `run N` is the editor's clock with no wall clock: it moves a
//! simulated clock exactly as far as N fixed steps take and hands it to the play core's
//! `PlayBackend::advance`, the call the shell makes every frame. **A simulation input is the bus's
//! `forge.play.input` session command** (`crate::play::input_command`, WP-19): an `input` line is
//! sugar for it, exactly as the GUI, a script and an automation session send it, and the runner
//! hands each one the bus delivers to the play core (gate `C-sim-input-command`). `sim-hash` reads
//! the play core directly (it changes nothing). One result line per request, then a summary with
//! the project's state hash and, when anything was simulated, the simulation's.
//!
//! [`HeadlessOptions`] add: `record` (write the last play session's replay file),
//! `replay` (replay a file after the script — against the script's project when a script
//! ran, checked by the recording's edit hash, else against the scene the file embeds — and
//! fail on the first divergence), `trace` (write a Perfetto trace of the run), and
//! `accept_project_security` (`--accept-project-security`: the launcher accepts the plugin
//! grants and default automation policy the script's projects carry, which a script's load
//! otherwise holds for a person, WP-33), and `trust_project` (`--trust-project`: the
//! launcher trusts the projects the run opens, which headless otherwise never does, WP-34).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer};
use forge_sim::{
    EditSnapshot, MAX_CATCH_UP, PlayCommand, PlaySession, PlayState, Recording, SIM_RATE_HZ,
    SimInput,
};
use forge_trace::Sinks;

use crate::EditorError;
use crate::client::{BusClient, SessionCommand};
use crate::core::{EditorCore, SharedCore};
use crate::play::{self, PlayBackend};
use crate::sim_bridge::SimPlay;

/// What a headless run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadlessSummary {
    pub requests: u64,
    pub applied_events: u64,
    pub refused: u64,
    pub entities: usize,
    pub settings: usize,
    pub state_hash: u64,
    /// Steps the last play session ran (0 if nothing was simulated).
    pub sim_steps: u64,
    /// The last play session's final state hash (its End hash after `stop`).
    pub sim_hash: Option<String>,
    /// A replay's result: events and hashes matched.
    pub replay: Option<(usize, usize)>,
}

/// Options beyond the script (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadlessOptions {
    /// Write the last play session's replay file here.
    pub record: Option<PathBuf>,
    /// Replay this file after the script.
    pub replay: Option<PathBuf>,
    /// Write a Perfetto trace of the run here.
    pub trace: Option<PathBuf>,
    /// `--accept-project-security`: the person who launched the run accepts, up front, the
    /// plugin grants and default automation policy the projects the script opens carry (WP-33).
    /// A script's load holds them like an automation session's (a script file may have been written
    /// by an automation session) and headless has no panel to accept them in; with this flag the
    /// runner accepts each held proposal right after the line that loaded it, as the human
    /// [`LAUNCHER`] — a flag on the command line no file can set. Off: they stay held (not in
    /// effect) and a save leaves them on disk as the files hold them.
    pub accept_project_security: bool,
    /// `--trust-project` (WP-34, [`crate::trust`]): the person who launched the run trusts
    /// the projects it opens. Headless has no one to ask (except the `--remote-host` console's
    /// person, who answers with `trust ID`, WP-36), so without it no project is
    /// trusted: the plugin grants a project carries stay held (and on disk as the files hold
    /// them). A flag on the command line, which no script or project file can set. It does
    /// not accept what a script's own open holds (`accept_project_security` does that).
    pub trust_project: bool,
    /// W2 positive control of `test_sim_input_command`: hand `input` lines to the play core
    /// directly, as before WP-19, instead of sending them on the bus. **Never set.**
    #[doc(hidden)]
    pub direct_inputs_for_tests: bool,
}

/// The person a `--accept-project-security` run accepts held security settings as.
pub const LAUNCHER: &str = "launcher (--accept-project-security)";

/// Accept the held security proposal, if any, as [`LAUNCHER`]: the line's report, `Err` for
/// a refusal.
fn accept_held(core: &SharedCore) -> Option<Result<String, String>> {
    let h = EditorCore::held_security(core)?;
    let mut person = EditorCore::connect(
        core,
        Issuer::Human {
            user: LAUNCHER.into(),
        },
    );
    person.apply(crate::security::accept_held_command(&h), None);
    Some(match person.pump().refused.first() {
        Some(r) => Err(format!("accepting held security settings: {}", r.rejection)),
        None => Ok(format!(
            "accepted {} held security setting(s) (--accept-project-security)",
            h.items.len()
        )),
    })
}

fn io(e: std::io::Error) -> EditorError {
    EditorError::Io(format!("headless: {e}"))
}

fn play_err(e: forge_sim::SimError) -> EditorError {
    EditorError::Play(e.to_string())
}

/// Run a script against `core` as `Script { path: script_name }`, writing one line per
/// request to `out`.
pub fn run_script(
    core: &SharedCore,
    script_name: &str,
    input: impl BufRead,
    out: &mut impl Write,
) -> Result<HeadlessSummary, EditorError> {
    run(
        core,
        script_name,
        Some(input),
        &HeadlessOptions::default(),
        out,
    )
}

/// A line that spells a play control or a simulation input: the `forge.play.control` or
/// `forge.play.input` command it stands for. `None`: not such a line.
fn play_sugar(t: &str) -> Option<Result<EditorCommand, String>> {
    let (word, rest) = t.split_once(' ').unwrap_or((t, ""));
    if word == "input" {
        return Some(
            serde_json::from_str::<serde_json::Value>(rest.trim())
                .map_err(|e| format!("not a simulation input: {e}"))
                .and_then(|v| play::parse_input_args(&v))
                .map(play::input_command),
        );
    }
    let cmd = match word {
        "play" => Ok(PlayCommand::Play),
        "pause" => Ok(PlayCommand::Pause),
        "stop" => Ok(PlayCommand::Stop),
        "step" => rest
            .trim()
            .parse::<u32>()
            .map(PlayCommand::Step)
            .map_err(|e| format!("`step N` needs a step count: {e}")),
        _ => return None,
    };
    Some(cmd.map(play::play_command))
}

/// The play half of a headless run.
struct Play {
    core: SimPlay,
    /// The editor's clock, simulated (see the module docs).
    now: Duration,
    /// Where the play core's current run of steps started on `now`, and the steps it has
    /// taken since; `None` until the next `run` anchors it (every control re-anchors: a
    /// resumed simulation restarts its clock run).
    anchor: Option<(Duration, u64)>,
    /// The pre-WP-19 direct input route (positive control only).
    direct_inputs: bool,
}

impl Play {
    /// Run a session command the bus delivered.
    fn session_command(
        &mut self,
        core: &SharedCore,
        sc: &SessionCommand,
    ) -> Result<String, String> {
        match play::decode(&sc.target, &sc.args) {
            Some(Ok(cmd)) => self.control(core, cmd),
            Some(Err(why)) => Err(format!("{}: {why}", sc.target)),
            None => match play::decode_input(&sc.target, &sc.args) {
                Some(Ok(i)) => {
                    PlayBackend::input(&mut self.core, i)?;
                    Ok(format!(
                        "input queued at step {}",
                        self.core.session().step()
                    ))
                }
                Some(Err(why)) => Err(format!("{}: {why}", sc.target)),
                None => Err(format!(
                    "{}: no session handler in a headless run",
                    sc.target
                )),
            },
        }
    }

    fn control(&mut self, core: &SharedCore, cmd: PlayCommand) -> Result<String, String> {
        let before = self.core.state();
        let at = self.core.session().step();
        // Only a stopped session forks: the project is read only when it is used.
        let edit = if before == PlayState::Stopped {
            EditorCore::read(core, EditSnapshot::from_project)
        } else {
            EditSnapshot::default()
        };
        self.core
            .control_edit(cmd, &edit)
            .map_err(|e| e.to_string())?;
        self.anchor = None;
        let after = self.core.state();
        let s = self.core.session();
        Ok(match (cmd, before == after) {
            (PlayCommand::Step(_), _) => format!("{after:?} at step {}", s.step()),
            (_, true) => format!("nothing to do ({after:?})"),
            (PlayCommand::Stop, false) => format!(
                "Stopped after {at} step(s), sim hash {}",
                s.recording().and_then(Recording::final_hash).unwrap_or("-")
            ),
            _ => format!("{after:?} at step {}", s.step()),
        })
    }

    /// `run N`: move the simulated clock exactly as far as N fixed steps take, in batches
    /// the play core's catch-up allows, through `PlayBackend::advance`.
    fn run(&mut self, n: u32) -> Result<String, String> {
        if self.core.state() != PlayState::Playing {
            return Err("`run` needs a playing simulation (`play` first)".into());
        }
        let (origin, mut taken) = match self.anchor {
            Some(a) => a,
            None => {
                // The first advance of a clock run anchors it at `now` and runs nothing.
                let ran = PlayBackend::advance(&mut self.core, self.now);
                if let Some(e) = self.core.take_error() {
                    return Err(e);
                }
                if ran != 0 {
                    return Err(format!("the play core ran {ran} step(s) at its anchor"));
                }
                (self.now, 0)
            }
        };
        let mut left = n;
        while left > 0 {
            let k = left.min(MAX_CATCH_UP);
            taken += u64::from(k);
            // ceil(taken / rate) s after the origin: exactly `taken` steps are due there.
            let micros = (u128::from(taken) * 1_000_000).div_ceil(u128::from(SIM_RATE_HZ));
            self.now = origin + Duration::from_micros(u64::try_from(micros).unwrap_or(u64::MAX));
            self.anchor = Some((origin, taken));
            let ran = PlayBackend::advance(&mut self.core, self.now);
            if let Some(e) = self.core.take_error() {
                return Err(e);
            }
            if ran != k {
                return Err(format!("the play core ran {ran} of {k} due step(s)"));
            }
            left -= k;
        }
        Ok(format!(
            "ran {n} step(s), at step {}",
            self.core.session().step()
        ))
    }

    /// A line the play core takes directly; `None` if the line is not one.
    fn direct(&mut self, t: &str) -> Option<Result<String, String>> {
        let (word, rest) = t.split_once(' ').unwrap_or((t, ""));
        let rest = rest.trim();
        Some(match word {
            "run" => rest
                .parse::<u32>()
                .map_err(|e| format!("`run N` needs a step count: {e}"))
                .and_then(|n| self.run(n)),
            // The pre-WP-19 route (positive control only): past the bus.
            "input" if self.direct_inputs => serde_json::from_str::<SimInput>(rest)
                .map_err(|e| format!("not a simulation input: {e}"))
                .and_then(|i| {
                    self.core.input(i).map_err(|e| e.to_string())?;
                    Ok(format!(
                        "input queued at step {}",
                        self.core.session().step()
                    ))
                }),
            "sim-hash" => Ok(match self.core.session().state_hash() {
                Some(h) => format!("sim hash {h} at step {}", self.core.session().step()),
                None => "no simulation".into(),
            }),
            _ => return None,
        })
    }
}

/// Run a headless session: the script (if any), then the options (see the module docs).
pub fn run(
    core: &SharedCore,
    script_name: &str,
    input: Option<impl BufRead>,
    opts: &HeadlessOptions,
    out: &mut impl Write,
) -> Result<HeadlessSummary, EditorError> {
    if opts.trust_project {
        // The launcher vouches for the projects this run opens (WP-34): their plugin grants
        // are not held for want of trust (a script's open still holds what it would widen).
        EditorCore::set_trust_book(core, std::sync::Arc::new(crate::trust::TrustEvery));
    }
    let tracer = forge_trace::global();
    if opts.trace.is_some() {
        tracer
            .enable(Sinks::PERFETTO.union(Sinks::COUNTERS))
            .map_err(|e| EditorError::Io(e.to_string()))?;
    }
    let mut client = EditorCore::connect(
        core,
        Issuer::Script {
            path: script_name.to_string(),
        },
    );
    // The runner hosts the play core: it follows the session commands (the play controls).
    client.follow_session(true);
    let mut sim = Play {
        core: SimPlay::with_tracer(tracer),
        now: Duration::ZERO,
        anchor: None,
        direct_inputs: opts.direct_inputs_for_tests,
    };
    // Outcomes from before this run are not this run's.
    let mut seen_outcome = client
        .pump()
        .project
        .and_then(|s| s.outcomes.last().map(|o| o.id))
        .unwrap_or(0);
    let mut sum = HeadlessSummary::default();
    let mut ran_script = false;
    if let Some(input) = input {
        ran_script = true;
        for (n, line) in input.lines().enumerate() {
            let line = line.map_err(io)?;
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            sum.requests += 1;
            if let Some(r) = sim.direct(t) {
                match r {
                    Ok(msg) => writeln!(out, "line {}: {msg}", n + 1).map_err(io)?,
                    Err(e) => {
                        sum.refused += 1;
                        writeln!(out, "line {}: refused {e}", n + 1).map_err(io)?;
                    }
                }
                continue;
            }
            let asked = match t {
                "undo" => client.undo_target().map(|txn| client.undo(txn)),
                "redo" => client.redo_target().map(|txn| client.redo(txn)),
                other => {
                    let cmd = match play_sugar(other) {
                        Some(sugar) => sugar,
                        None => serde_json::from_str::<EditorCommand>(other)
                            .map_err(|e| format!("not a command: {e}")),
                    };
                    match cmd {
                        Ok(cmd) => Some(client.apply(cmd, None)),
                        Err(e) => {
                            sum.refused += 1;
                            writeln!(out, "line {}: refused {e}", n + 1).map_err(io)?;
                            continue;
                        }
                    }
                }
            };
            // A script is sequential: a push, pull or clone the line started finishes (off
            // the core's lock, so other clients keep working) before the next line runs.
            EditorCore::wait_transfers(core);
            let p = client.pump();
            sum.applied_events += p.events.len() as u64;
            match (asked, p.refused.first()) {
                (None, _) => writeln!(out, "line {}: nothing to {t}", n + 1).map_err(io)?,
                (Some(_), Some(r)) => {
                    sum.refused += 1;
                    writeln!(out, "line {}: refused {}", n + 1, r.rejection).map_err(io)?;
                }
                // A session command the bus accepted: the play core's result is the line's.
                (Some(_), None) if !p.session.is_empty() => {
                    for sc in &p.session {
                        match sim.session_command(core, sc) {
                            Ok(msg) => writeln!(out, "line {}: {msg}", n + 1).map_err(io)?,
                            Err(e) => {
                                sum.refused += 1;
                                writeln!(out, "line {}: refused {e}", n + 1).map_err(io)?;
                            }
                        }
                    }
                }
                (Some(_), None) => {
                    let changes: usize = p.events.iter().map(|e| e.diff.len()).sum();
                    writeln!(out, "line {}: ok ({changes} change(s))", n + 1).map_err(io)?;
                }
            }
            if p.session_dropped > 0 {
                writeln!(
                    out,
                    "line {}: {} session command(s) dropped",
                    n + 1,
                    p.session_dropped
                )
                .map_err(io)?;
            }
            // Project lifecycle commands (create, open, save, push, pull, build): the core
            // performed them; say what happened, as the GUI's toasts do.
            if let Some(st) = &p.project {
                let me = client.issuer().tag();
                let from = seen_outcome;
                for o in st.outcomes.iter().filter(|o| o.id > from) {
                    seen_outcome = o.id;
                    if o.issuer != me {
                        continue;
                    }
                    match &o.result {
                        Ok(msg) => writeln!(out, "line {}: {}: {msg}", n + 1, o.op).map_err(io)?,
                        Err((_, msg)) => {
                            sum.refused += 1;
                            writeln!(out, "line {}: {} failed: {msg}", n + 1, o.op).map_err(io)?;
                        }
                    }
                }
            }
            // The launcher accepted, up front, what the projects carry (WP-33): before the
            // next line runs (a build, a play session) the held settings take effect.
            if opts.accept_project_security
                && let Some(r) = accept_held(core)
            {
                match r {
                    Ok(msg) => writeln!(out, "line {}: {msg}", n + 1).map_err(io)?,
                    Err(e) => {
                        sum.refused += 1;
                        writeln!(out, "line {}: refused {e}", n + 1).map_err(io)?;
                    }
                }
            }
        }
    }
    let rec = sim.core.session().recording().cloned();
    sum.sim_steps = rec.as_ref().map_or(0, Recording::steps);
    sum.sim_hash = sim
        .core
        .session()
        .state_hash()
        .or_else(|| rec.as_ref().and_then(|r| r.final_hash().map(str::to_owned)));
    if let Some(path) = &opts.record {
        match &rec {
            Some(r) => {
                r.write(path).map_err(play_err)?;
                writeln!(
                    out,
                    "recorded {} event(s), {} step(s) to {}",
                    r.events.len(),
                    r.steps(),
                    path.display()
                )
                .map_err(io)?;
            }
            None => writeln!(out, "nothing was played: no replay file written").map_err(io)?,
        }
    }
    if let Some(path) = &opts.replay {
        let rec = Recording::read(path).map_err(play_err)?;
        let scene = ran_script.then(|| EditorCore::read(core, EditSnapshot::from_project));
        let report = forge_sim::replay(&rec, scene.as_ref(), PlaySession::with_tracer(tracer))
            .map_err(play_err)?;
        writeln!(
            out,
            "replay: {} event(s), {} hash(es) matched over {} step(s), bit-for-bit ({})",
            report.events,
            report.checks,
            report.steps,
            if scene.is_some() {
                "against the script's project"
            } else {
                "against the recorded scene"
            }
        )
        .map_err(io)?;
        sum.sim_steps = report.steps;
        sum.sim_hash = Some(report.final_hash);
        sum.replay = Some((report.events, report.checks));
    }
    if let Some(path) = &opts.trace {
        tracer.frame_mark();
        let n = tracer
            .write_perfetto(path, "forge --headless")
            .map_err(|e| EditorError::Io(e.to_string()))?;
        for (sub, budgets) in tracer.budgets_by_subsystem() {
            for b in budgets {
                writeln!(
                    out,
                    "budget {sub}: {} peak {:.3}{} of {:.3}{}, over in {} of {} frame(s)",
                    b.name, b.peak, b.unit, b.budget, b.unit, b.over_frames, b.samples
                )
                .map_err(io)?;
            }
        }
        writeln!(out, "trace: {n} bytes to {} (Perfetto)", path.display()).map_err(io)?;
    }
    let (entities, settings, state_hash) =
        EditorCore::read(core, |p| (p.len(), p.settings().len(), p.state_hash()));
    sum.entities = entities;
    sum.settings = settings;
    sum.state_hash = state_hash;
    writeln!(
        out,
        "summary: {} request(s), {} event(s), {} refused, {} entit(ies), {} setting(s), state hash {:016x}",
        sum.requests, sum.applied_events, sum.refused, sum.entities, sum.settings, sum.state_hash
    )
    .map_err(io)?;
    if let Some(h) = &sum.sim_hash {
        writeln!(out, "sim hash {h} after {} step(s)", sum.sim_steps).map_err(io)?;
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_text(script: &str) -> (HeadlessSummary, String) {
        let core = EditorCore::new();
        let mut out = Vec::new();
        let s = run_script(&core, "t.forge", script.as_bytes(), &mut out)
            .unwrap_or_else(|e| panic!("{e}"));
        (s, String::from_utf8(out).unwrap_or_default())
    }

    #[test]
    fn a_script_runs_through_a_client_and_reports_refusals() {
        let script = "# make a cube\n\
            {\"Spawn\":{\"name\":\"Cube\",\"parent\":null}}\n\
            {\"SetSetting\":{\"key\":\"editor.grid_size\",\"value\":{\"Float\":2.0}}}\n\
            {\"Rename\":{\"entity\":99,\"name\":\"x\"}}\n\
            undo\n\
            redo\n\
            not json\n";
        let (s, text) = run_text(script);
        assert_eq!(s.requests, 6, "{text}");
        assert_eq!(s.refused, 2, "{text}");
        assert_eq!(s.entities, 1);
        assert_eq!(s.settings, 1, "undo then redo leaves the setting: {text}");
        assert!(text.contains("refused CMD-"), "{text}");
        assert!(text.contains("summary:"));
        assert_eq!(s.sim_hash, None);
    }

    const BALL: &str = "{\"Spawn\":{\"name\":\"Ball\",\"parent\":null}}\n\
        {\"SetProperty\":{\"entity\":0,\"path\":\"transform.position.local\",\"value\":{\"Vec3\":[0.0,10.0,0.0]}}}\n\
        {\"SetProperty\":{\"entity\":0,\"path\":\"motion.acceleration\",\"value\":{\"Vec3\":[0.0,-9.81,0.0]}}}\n";

    #[test]
    fn play_lines_fork_the_project_step_and_stop() {
        let script = format!(
            "{BALL}run 5\n\
            play\n\
            run 30\n\
            input {{\"entity\":0,\"action\":{{\"Impulse\":[0.0,5.0,0.0]}}}}\n\
            input {{\"entity\":7,\"action\":{{\"SetSpin\":1.0}}}}\n\
            pause\n\
            step 3\n\
            sim-hash\n\
            stop\n"
        );
        let (s, text) = run_text(&script);
        assert_eq!(
            s.refused, 2,
            "`run` before play and an unknown entity: {text}"
        );
        assert!(text.contains("refused SIM-0002"), "{text}");
        assert_eq!(s.sim_steps, 33, "{text}");
        assert!(s.sim_hash.is_some());
        assert!(text.contains("Stopped after 33 step(s)"), "{text}");
        assert_eq!(s.entities, 1, "playing never changes the project");
    }

    #[test]
    fn a_play_control_line_is_the_bus_command_the_gui_sends() {
        // The sugar and the command in its wire form (what the GUI's panel, an automation session
        // or a script sends) run the same session: same steps, same simulation hash.
        let sugar = format!("{BALL}play\nrun 40\nstep 2\nstop\n");
        let control = |c: &str| {
            let cmd = match c {
                "play" => PlayCommand::Play,
                "stop" => PlayCommand::Stop,
                _ => PlayCommand::Step(2),
            };
            serde_json::to_string(&play::play_command(cmd)).unwrap_or_default()
        };
        let wire = format!(
            "{BALL}{}\nrun 40\n{}\n{}\n",
            control("play"),
            control("step"),
            control("stop")
        );
        let (a, ta) = run_text(&sugar);
        let (b, tb) = run_text(&wire);
        assert_eq!(a.refused, 0, "{ta}");
        assert_eq!(b.refused, 0, "{tb}");
        assert_eq!(a.sim_steps, 42, "{ta}");
        assert_eq!(
            (b.sim_steps, &b.sim_hash),
            (a.sim_steps, &a.sim_hash),
            "{ta}\n{tb}"
        );
        // Step while playing pauses first, as the toolbar does.
        assert!(ta.contains("Paused at step 42"), "{ta}");
        // A session command is no project edit: nothing entered the undo history.
        assert_eq!(a.applied_events, b.applied_events);
    }

    #[test]
    fn a_malformed_play_control_is_refused_by_the_bus_and_never_runs() {
        let script = format!(
            "{BALL}{}\nstep 0\nplay\n",
            serde_json::to_string(&EditorCommand::Invoke {
                target: play::PLAY_CMD.into(),
                args: "{\"control\":\"rewind\"}".into(),
            })
            .unwrap_or_default()
        );
        let (s, text) = run_text(&script);
        assert_eq!(s.refused, 2, "{text}");
        assert!(text.contains("line 4: refused CMD-"), "{text}");
        assert!(text.contains("line 5: refused CMD-"), "{text}");
        assert!(text.contains("line 6: Playing at step 0"), "{text}");
    }

    #[test]
    fn run_moves_the_clock_exactly_n_steps_across_catch_up_batches() {
        let script = format!("{BALL}play\nrun 1\nrun 16\nrun 1000\npause\nplay\nrun 7\nstop\n");
        let (s, text) = run_text(&script);
        assert_eq!(s.refused, 0, "{text}");
        assert!(text.contains("ran 1000 step(s), at step 1017"), "{text}");
        assert_eq!(s.sim_steps, 1024, "{text}");
    }

    #[test]
    fn a_script_creates_saves_links_and_pushes_a_project() {
        let core = EditorCore::new();
        let script = r#"{"Invoke":{"target":"forge.project.create","args":"{\"location\":\"memory:headless-proj\",\"name\":\"Headless\",\"template\":\"3d\"}"}}
{"Spawn":{"name":"Crate","parent":null}}
{"Invoke":{"target":"forge.project.save","args":"{\"message\":\"first\"}"}}
{"Invoke":{"target":"forge.project.push","args":"{}"}}
{"Invoke":{"target":"forge.project.link_remote","args":"{\"url\":\"git@github.com:a/b.git\"}"}}
{"Invoke":{"target":"forge.project.link_remote","args":"{\"url\":\"memory:headless-remote\"}"}}
{"Invoke":{"target":"forge.project.push","args":"{}"}}
{"Invoke":{"target":"forge.export.build","args":"{\"target\":\"linux\"}"}}
"#;
        let mut out = Vec::new();
        let s = run_script(&core, "p.forge", script.as_bytes(), &mut out)
            .unwrap_or_else(|e| panic!("{e}"));
        let text = String::from_utf8(out).unwrap_or_default();
        assert!(text.contains("forge.project.create: Created"), "{text}");
        assert!(
            text.contains("forge.project.save: Saved revision"),
            "{text}"
        );
        assert!(
            text.contains("forge.project.push failed: PROJECT-0005"),
            "push with no remote is refused with its code: {text}"
        );
        assert!(
            text.contains("https://github.com/a/b.git"),
            "an SSH Git remote is refused with its HTTPS form: {text}"
        );
        assert!(text.contains("Pushed 1 revision(s)"), "{text}");
        assert!(text.contains("forge.export.build: Built"), "{text}");
        assert_eq!(s.refused, 2, "{text}");
        let st = EditorCore::project_status(&core);
        let open = st.open.as_ref().unwrap_or_else(|| panic!("open"));
        assert_eq!(open.name, "Headless");
        assert_eq!(open.revisions.len(), 1);
        assert!(forge_project::memory::exists("headless-remote"));
    }
}
