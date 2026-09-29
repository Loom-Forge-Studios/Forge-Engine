//! Guards (Ch.28 §28.18, owner rule 2): forge-input's budget rows (`budgets.ron`), each
//! measured here.
//!
//! * `C-input-latency-budget` (`input.latency.frames` = 0): an event pushed before an update
//!   shows in that update's action state, for every event path (a key, a pad button, a stick,
//!   a touch gesture, an on-screen button). Control: events applied one update late.
//! * `C-input-alloc-free-frame` (`input.update.alloc_bytes` = 0): a steady frame of the
//!   reference game (4 players, 48 actions, 40 events) allocates nothing. Control: a scratch
//!   buffer per frame.
//! * `C-input-idle-cost` (`input.update.idle`): a runtime nothing feeds reads no binding and
//!   costs at most its allowance per update; a map with no player is not evaluated.
//!   Control: evaluating the map with no player.
//! * `C-input-update-budget` (`input.update`): one frame of the reference game within its
//!   allowance. Control: a runtime that looks each control up by name on every read.
//!
//! Timed bodies run alone (`forge_trace::timed::run_timed_alone`: a process of their own,
//! the machine-wide timed lock, a quiet machine) and report the minimum over several trials
//! (preemption only adds time), so the guards do not flake under a parallel test run (W5).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Instant;

use common::*;
use forge_input::{
    ActionDef, ActionKind, Binding, ContextDef, Device, DeviceDesc, InputFaults, InputMapDef,
    InputRuntime, JoinPolicy, OnScreenControls, PlayerId, TouchInput, TouchPhase, Trigger,
};

type Check = Result<(), String>;
const P0: PlayerId = PlayerId(0);

fn allowance(name: &str) -> f64 {
    forge_input::budget(name).unwrap_or_else(|| panic!("no budget row {name}"))
}

// ---- latency ----------------------------------------------------------------------------------

/// Frames from each event to its action (the worst over every path).
fn latency_frames(f: InputFaults) -> u64 {
    let t = |n: &str| Binding::Control(c(Device::Touch, n));
    let map = InputMapDef::new().context(
        ContextDef::new("g")
            .action(
                ActionDef::new("key", ActionKind::Button)
                    .bind(bd(key("Space")))
                    .trigger(Trigger::Pressed),
            )
            .action(
                ActionDef::new("pad", ActionKind::Button)
                    .bind(bd(pad("North")))
                    .trigger(Trigger::Pressed),
            )
            .action(ActionDef::new("stick", ActionKind::Axis2D).bind(bd(pad("RightStick"))))
            .action(ActionDef::new("tap", ActionKind::Button).bind(bd(t("Tap"))))
            .action(ActionDef::new("south", ActionKind::Button).bind(bd(pad("South")))),
    );
    let mut rt = InputRuntime::new().with_faults(f);
    rt.set_map(&map);
    let kb = virtual_keyboard(&mut rt);
    let p = virtual_pad(&mut rt, "p");
    let ts = rt.connect(DeviceDesc::new(Device::Touch, "t", "t").virtual_device());
    let mut os = OnScreenControls::new(&mut rt, OnScreenControls::default_layout());
    frames(&mut rt, 3);
    let mut worst = 0u64;
    let mut measure =
        |rt: &mut InputRuntime, path: &str, fire: &mut dyn FnMut(&mut InputRuntime)| {
            let h = rt.handle(path).unwrap();
            fire(rt);
            let mut n = 0u64;
            loop {
                rt.update(DT);
                let s = rt.action(P0, h);
                if s.triggered || s.value != [0.0, 0.0] {
                    break;
                }
                n += 1;
                assert!(n < 10, "{path} never fired");
            }
            worst = worst.max(n);
            frames(rt, 5);
        };
    measure(&mut rt, "g/key", &mut |rt| rt.send(kb, "Space", [1.0, 0.0]));
    measure(&mut rt, "g/pad", &mut |rt| rt.send(p, "North", [1.0, 0.0]));
    measure(&mut rt, "g/stick", &mut |rt| {
        rt.send(p, "RightStick", [0.7, 0.0])
    });
    measure(&mut rt, "g/tap", &mut |rt| {
        rt.touch(ts, TouchInput::new(1, TouchPhase::Started, 10.0, 10.0));
        rt.touch(ts, TouchInput::new(1, TouchPhase::Ended, 10.0, 10.0));
    });
    let south = os.center(1);
    measure(&mut rt, "g/south", &mut |rt| {
        os.touch(
            rt,
            TouchInput::new(9, TouchPhase::Started, south[0], south[1]),
        );
    });
    worst
}

fn latency_check(f: InputFaults) -> Check {
    let n = latency_frames(f);
    let budget = allowance("input.latency.frames");
    if n as f64 <= budget {
        Ok(())
    } else {
        Err(format!(
            "input.latency.frames: {n} frames (allowance {budget})"
        ))
    }
}

#[test]
fn an_event_shows_in_the_same_update() {
    latency_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_events_a_frame_late_fail() {
    let e = latency_check(InputFaults {
        defer_events: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("1 frames"), "{e}");
}

// ---- allocation -------------------------------------------------------------------------------

/// Bytes allocated by 60 steady frames of the reference game (after a warm-up).
fn steady_alloc(f: InputFaults) -> u64 {
    let (mut rt, devs) = reference_runtime(f);
    for frame in 0..120 {
        reference_events(&mut rt, &devs, frame);
        rt.update(DT);
    }
    let info = allocation_counter::measure(|| {
        for frame in 120..180 {
            reference_events(&mut rt, &devs, frame);
            rt.update(DT);
        }
    });
    assert!(rt.stats().bindings_read > 0);
    info.bytes_total
}

#[test]
fn a_steady_frame_allocates_nothing() {
    let b = steady_alloc(InputFaults::default());
    assert!(
        b as f64 <= allowance("input.update.alloc_bytes"),
        "60 steady frames allocated {b} bytes"
    );
}

#[test]
fn positive_control_a_scratch_buffer_per_frame_fails() {
    let b = steady_alloc(InputFaults {
        alloc_per_frame: true,
        ..InputFaults::default()
    });
    assert!(b as f64 > allowance("input.update.alloc_bytes"), "{b}");
}

// ---- idle ---------------------------------------------------------------------------------------

fn min_ns_per_update(rt: &mut InputRuntime, iters: u32) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..9 {
        let t0 = Instant::now();
        for _ in 0..iters {
            rt.update(DT);
        }
        best = best.min(t0.elapsed().as_nanos() as f64 / f64::from(iters));
    }
    best
}

/// An unused runtime: none fed, no map; and a map with no player (the menu of a game whose
/// players have not joined).
fn idle_runtimes(f: InputFaults) -> (InputRuntime, InputRuntime) {
    let unused = InputRuntime::new().with_faults(f);
    let mut waiting = InputRuntime::new().with_faults(f);
    waiting.set_join_policy(JoinPolicy::Manual);
    waiting.set_map(&reference_map());
    (unused, waiting)
}

fn idle_check(f: InputFaults) -> Check {
    let (mut unused, mut waiting) = idle_runtimes(f);
    frames(&mut unused, 100);
    frames(&mut waiting, 100);
    let read = unused.stats().bindings_read + waiting.stats().bindings_read;
    if read == 0 {
        Ok(())
    } else {
        Err(format!("an idle runtime read {read} bindings"))
    }
}

#[test]
fn an_unused_runtime_evaluates_nothing() {
    idle_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_evaluating_with_no_player_fails() {
    assert!(
        idle_check(InputFaults {
            eval_without_players: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

fn timed_idle(f: InputFaults) -> Result<String, String> {
    forge_trace::timed::run_timed_alone(move || {
        let (mut unused, mut waiting) = idle_runtimes(f);
        let a = min_ns_per_update(&mut unused, 200_000);
        let b = min_ns_per_update(&mut waiting, 20_000);
        let worst = a.max(b);
        let budget = allowance("input.update.idle");
        let line = format!(
            "input.update.idle: unused {a:.1} ns, map with no player {b:.1} ns (allowance {budget} ns)"
        );
        if worst <= budget { Ok(line) } else { Err(line) }
    })
}

#[test]
fn an_unused_runtime_costs_its_idle_budget() {
    let s = timed_idle(InputFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    println!("{s}");
}

#[test]
fn positive_control_evaluating_with_no_player_exceeds_the_idle_budget() {
    let e = timed_idle(InputFaults {
        eval_without_players: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    println!("{e}");
    assert!(e.contains("input.update.idle"), "{e}");
}

// ---- the reference frame ------------------------------------------------------------------------

fn timed_update(f: InputFaults) -> Result<String, String> {
    forge_trace::timed::run_timed_alone(move || {
        let (mut rt, devs) = reference_runtime(f);
        for frame in 0..120 {
            reference_events(&mut rt, &devs, frame);
            rt.update(DT);
        }
        let mut best = f64::INFINITY;
        let mut frame = 120u64;
        for _ in 0..9 {
            let mut total = 0.0f64;
            for _ in 0..200 {
                reference_events(&mut rt, &devs, frame);
                frame += 1;
                let t0 = Instant::now();
                rt.update(DT);
                total += t0.elapsed().as_secs_f64();
            }
            best = best.min(total * 1e3 / 200.0);
        }
        let budget = allowance("input.update");
        let s = rt.stats();
        let line = format!(
            "input.update: {best:.4} ms per frame (allowance {budget} ms; {} bindings and {} actions a frame)",
            s.bindings_read / s.updates,
            s.actions_evaluated / s.updates
        );
        if best <= budget { Ok(line) } else { Err(line) }
    })
}

#[test]
fn the_reference_frame_fits_its_budget() {
    let s = timed_update(InputFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    println!("{s}");
}

#[test]
fn positive_control_name_lookups_per_read_exceed_the_budget() {
    let e = timed_update(InputFaults {
        lookup_by_name: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    println!("{e}");
    assert!(e.contains("input.update"), "{e}");
}

#[test]
fn the_budgets_reach_the_profiler() {
    let t = forge_trace::Tracer::new();
    assert_eq!(forge_input::declare_budgets(&t), 4);
    let b = t.budgets_by_subsystem();
    assert!(
        b.get("input")
            .is_some_and(|rows| rows.iter().any(|r| r.name == "input.update"))
    );
}
