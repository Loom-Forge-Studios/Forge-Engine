//! `test_play_trace_backends` (M2-5, M2-11; Ch.21 §21.21 "Play controls" and "Profiler",
//! Ch.29): the editor's play controls run on WP-13's play core and its profiler reads
//! `forge-trace` — the real backends behind WP-U6's `PlayBackend` and `CounterSource`.
//!
//! * The default services wire them: `play` is the `forge-sim` session (and `play_core` is
//!   that same object), `profiler` is `forge-trace`'s live feed over the process tracer the
//!   play core reports to.
//! * In a running shell, the play controls sent as `forge.play.control` bus commands step the
//!   play core, and the profiler the panel reads shows those steps: `sim.step` frame parts,
//!   the `sim.step` budget measured, the `sim.bodies` counter, and a moved generation.
//!
//! Positive control (W2): `positive_control_a_profiler_not_reading_the_tracer_fails` — a shell
//! whose profiler is a source the play core does not report to (the in-memory
//! `MemoryCounters`) must fail the check.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::play::{PlayCommand, PlayState};
use forge_editor::presets::builtin_preset;
use forge_editor::profile::MemoryCounters;
use forge_editor::services::EditorServices;
use forge_editor::shell::assemble;
use forge_editor::sim_bridge::{SimPlay, TraceCounters};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_sim::PlaySession;
use forge_trace::{Sinks, Tracer};

#[test]
fn the_default_services_run_the_play_core_and_read_forge_trace() {
    let s = EditorServices::default();
    assert_eq!(s.play.borrow().backend(), PlaySession::BACKEND);
    assert_eq!(s.profiler.backend(), TraceCounters::BACKEND);
    let core = s.play_core.clone().expect("the play core is exposed");
    assert_eq!(
        Rc::as_ptr(&s.play).cast::<u8>(),
        Rc::as_ptr(&core).cast::<u8>(),
        "play and play_core are one object"
    );
    assert!(std::ptr::eq(s.tracer, forge_trace::global()));
    assert!(std::ptr::eq(core.borrow().session().tracer(), s.tracer));
    assert!(
        s.profiler
            .snapshot()
            .budgets
            .iter()
            .any(|b| b.name == "sim.step"),
        "the play core's budget is declared on the tracer the profiler reads"
    );
}

/// Play a ball through the shell's own play controls; check the profiler saw the steps.
fn check(profiler_reads_tracer: bool) -> Result<(), String> {
    let tracer: &'static Tracer = Box::leak(Box::new(Tracer::new()));
    tracer.enable(Sinks::COUNTERS).unwrap();
    let stand_in = StandInPanels::without(&[]).unwrap();
    let mut cfg = assemble(builtin_preset("3d").unwrap(), &[&stand_in], &[], None).unwrap();
    let mut services =
        std::mem::take(&mut cfg.services).with_play_core(SimPlay::with_tracer(tracer));
    services.tracer = tracer;
    services.profiler = if profiler_reads_tracer {
        Arc::new(TraceCounters::of(tracer))
    } else {
        Arc::new(MemoryCounters::new())
    };
    cfg.services = services;
    let mut rig = Rig::new(cfg).unwrap();
    let em = rig.shell.emitter().clone();
    em.emit(EditorCommand::Spawn {
        name: "Ball".into(),
        parent: None,
    });
    em.emit(EditorCommand::SetProperty {
        entity: EntityKey(0),
        path: "transform.position.local".into(),
        value: Value::Vec3([0.0, 1.0, 0.0]),
    });
    em.emit(EditorCommand::SetProperty {
        entity: EntityKey(0),
        path: "motion.velocity".into(),
        value: Value::Vec3([2.0, 0.0, 0.0]),
    });
    rig.settle();
    let s = rig.shell.handles().services_rc();
    let g0 = s.profiler.generation();

    rig.shell.play(PlayCommand::Step(5));
    rig.shell.play(PlayCommand::Play);
    rig.advance(Duration::from_millis(500));
    rig.shell.play(PlayCommand::Stop);
    if s.play.borrow().state() != PlayState::Stopped || s.play.borrow().log().len() != 3 {
        return Err(format!(
            "the controls did not run: {:?}",
            s.play.borrow().log()
        ));
    }

    let p = s.profiler.snapshot();
    if s.profiler.generation() == g0 {
        return Err("the profiler's feed never moved while the simulation ran".into());
    }
    let step_frames = p
        .frames
        .iter()
        .filter(|f| f.parts.iter().any(|(n, _)| n == "sim.step"))
        .count();
    if step_frames < 2 {
        return Err(format!(
            "the profiler shows {step_frames} frame(s) with sim.step (source: {})",
            s.profiler.backend()
        ));
    }
    let budget = p.budgets.iter().find(|b| b.name == "sim.step");
    if budget.is_none() {
        return Err("no sim.step budget".into());
    }
    if !p.counters.contains(&("sim.bodies".to_owned(), 1.0)) {
        return Err(format!("no sim.bodies counter: {:?}", p.counters));
    }
    Ok(())
}

#[test]
fn the_profiler_shows_what_the_play_core_measured() {
    check(true).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_profiler_not_reading_the_tracer_fails() {
    let e = check(false).expect_err("a profiler that does not read forge-trace must fail");
    assert!(e.contains("never moved") || e.contains("frame(s)"), "{e}");
}
