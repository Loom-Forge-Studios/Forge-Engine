//! Guards (Ch.28 §28.11–§28.12, DoD M7-12): **deadzones, response curves and triggers do
//! what their names promise**, measured through the whole runtime (virtual devices → raw
//! events → one `update` a frame at 60 Hz → action states), never by calling a helper
//! in isolation.
//!
//! * `C-input-deadzone-curve`: a radial deadzone cuts a small diagonal and rescales past its
//!   edge (the smallest movement past it is a small value, not a jump); response curves
//!   reshape the magnitude and keep a stick's direction; a digital WASD diagonal is
//!   normalised.
//! * `C-input-triggers`: Hold, Tap, MultiTap, Pulse, Chord, Combo, Pressed and Released fire
//!   when (and only when) they should, with the phase events that go with them; a tap faster
//!   than a frame is not lost; a higher-priority context consumes its controls.
//!
//! Positive controls (W2): each `positive_control_*` runs the identical check with one
//! `InputFaults` switch that breaks the property, and requires the check to fail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_input::{
    ActionDef, ActionKind, Binding, ContextDef, Curve, DeadzoneKind, Device, DeviceId, InputFaults,
    InputMapDef, InputRuntime, Modifier, Phase, PlayerId, Trigger,
};

type Check = Result<(), String>;

fn ensure(ok: bool, why: impl Into<String>) -> Check {
    if ok { Ok(()) } else { Err(why.into()) }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// A runtime with one context holding `actions`, one pad and one keyboard (single player).
fn rig(faults: InputFaults, actions: Vec<ActionDef>) -> (InputRuntime, DeviceId, DeviceId) {
    let mut ctx = ContextDef::new("g");
    for a in actions {
        ctx = ctx.action(a);
    }
    let mut rt = InputRuntime::new().with_faults(faults);
    let p = rt.set_map(&InputMapDef::new().context(ctx));
    assert!(p.is_empty(), "{p:?}");
    let pad = virtual_pad(&mut rt, "p");
    let kb = virtual_keyboard(&mut rt);
    (rt, pad, kb)
}

const P0: PlayerId = PlayerId(0);

// ---- deadzones and curves -------------------------------------------------------------------

fn deadzone_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("move", ActionKind::Axis2D).bind(bd(pad_b("LeftStick")).with(
                Modifier::Deadzone {
                    kind: DeadzoneKind::Radial,
                    lower: 0.15,
                    upper: 0.95,
                },
            )),
        ],
    );
    let mv = rt.handle("g/move").unwrap();
    let read = |rt: &mut InputRuntime, v: [f32; 2]| {
        rt.send(pad, "LeftStick", v);
        rt.update(DT);
        rt.action(P0, mv)
    };
    // Rest noise inside the deadzone reads zero and does not actuate.
    let s = read(&mut rt, [0.1, 0.05]);
    ensure(
        s.value == [0.0, 0.0] && s.phase == Phase::Idle,
        format!("noise read {s:?}"),
    )?;
    // A diagonal of magnitude 0.17 is past a radial deadzone of 0.15 (each axis is only 0.12).
    let s = read(&mut rt, [0.12, 0.12]);
    ensure(
        s.value[0] > 0.0 && close(s.value[0], s.value[1]),
        format!("the small diagonal was cut or bent: {:?}", s.value),
    )?;
    // Just past the edge reads just above zero: no jump.
    let s = read(&mut rt, [0.16, 0.0]);
    ensure(
        s.value[0] > 0.0 && s.value[0] < 0.02,
        format!("0.16 past a 0.15 deadzone read {} (a jump)", s.value[0]),
    )?;
    // Rescaled: 0.55 is half-way between 0.15 and 0.95.
    let s = read(&mut rt, [0.55, 0.0]);
    ensure(close(s.value[0], 0.5), format!("0.55 read {}", s.value[0]))?;
    // Past the outer edge reads full, direction kept.
    let s = read(&mut rt, [0.0, -0.99]);
    ensure(close(s.value[1], -1.0), format!("-0.99 read {:?}", s.value))?;
    Ok(())
}

fn pad_b(n: &str) -> Binding {
    pad(n)
}

#[test]
fn radial_deadzone_cuts_noise_and_rescales_without_a_jump() {
    deadzone_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_a_deadzone_that_does_not_rescale_fails() {
    let e = deadzone_check(InputFaults {
        deadzone_no_rescale: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("jump"), "{e}");
}

#[test]
fn positive_control_a_radial_deadzone_applied_per_axis_fails() {
    let e = deadzone_check(InputFaults {
        radial_as_axial: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("diagonal"), "{e}");
}

fn curve_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("aim", ActionKind::Axis2D)
                .bind(bd(pad("RightStick")).with(Modifier::Curve(Curve::Power(2.0)))),
            ActionDef::new("gas", ActionKind::Axis1D).bind(bd(pad("RightTrigger")).with(
                Modifier::Curve(Curve::Points(vec![(0.0, 0.0), (0.5, 0.1), (1.0, 1.0)])),
            )),
            ActionDef::new("brake", ActionKind::Axis1D)
                .bind(bd(pad("LeftTrigger")).with(Modifier::Curve(Curve::Smooth))),
        ],
    );
    let (aim, gas, brake) = (
        rt.handle("g/aim").unwrap(),
        rt.handle("g/gas").unwrap(),
        rt.handle("g/brake").unwrap(),
    );
    rt.send(pad, "RightStick", [0.3, 0.4]);
    rt.send(pad, "RightTrigger", [0.75, 0.0]);
    rt.send(pad, "LeftTrigger", [0.25, 0.0]);
    rt.update(DT);
    let a = rt.action(P0, aim).value;
    // |(0.3, 0.4)| = 0.5 -> 0.25, same direction (0.6, 0.8).
    ensure(
        close(a[0], 0.15) && close(a[1], 0.2),
        format!("power curve read {a:?}, want [0.15, 0.2]"),
    )?;
    let g = rt.action(P0, gas).value[0];
    ensure(
        close(g, 0.55),
        format!("point curve at 0.75 read {g}, want 0.55"),
    )?;
    let b = rt.action(P0, brake).value[0];
    ensure(close(b, 0.15625), format!("smoothstep at 0.25 read {b}"))?;
    Ok(())
}

#[test]
fn response_curves_reshape_the_magnitude_and_keep_the_direction() {
    curve_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_linear_curves_fail() {
    assert!(
        curve_check(InputFaults {
            curve_linear: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

#[test]
fn a_wasd_diagonal_is_normalised() {
    let (mut rt, _, kb) = rig(
        InputFaults::default(),
        vec![
            ActionDef::new("move", ActionKind::Axis2D).bind(bd(Binding::Composite2D([
                c(Device::Keyboard, "W"),
                c(Device::Keyboard, "S"),
                c(Device::Keyboard, "A"),
                c(Device::Keyboard, "D"),
            ]))),
        ],
    );
    let mv = rt.handle("g/move").unwrap();
    rt.send(kb, "W", [1.0, 0.0]);
    rt.send(kb, "D", [1.0, 0.0]);
    rt.update(DT);
    let v = rt.action(P0, mv).value;
    assert!(
        close(v[0], std::f32::consts::FRAC_1_SQRT_2) && close(v[1], v[0]),
        "{v:?}"
    );
}

// ---- triggers -------------------------------------------------------------------------------

/// Hold the pad's South for `secs`, then release; returns (frames triggered, canceled seen,
/// completed seen).
fn hold_for(
    rt: &mut InputRuntime,
    pad: DeviceId,
    a: forge_input::ActionHandle,
    secs: f32,
) -> (u32, bool, bool) {
    let mut triggered = 0;
    let (mut canceled, mut completed) = (false, false);
    rt.send(pad, "South", [1.0, 0.0]);
    let n = (secs / DT).round() as usize;
    for i in 0..=n {
        if i == n {
            rt.send(pad, "South", [0.0, 0.0]);
        }
        rt.update(DT);
        let s = rt.action(P0, a);
        triggered += u32::from(s.triggered);
        canceled |= s.canceled;
        completed |= s.completed;
    }
    frames(rt, 2);
    (triggered, canceled, completed)
}

fn hold_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("charge", ActionKind::Button)
                .bind(bd(pad("South")))
                .trigger(Trigger::Hold {
                    time: 0.5,
                    repeat: false,
                }),
        ],
    );
    let a = rt.handle("g/charge").unwrap();
    let (t, canceled, _) = hold_for(&mut rt, pad, a, 0.3);
    ensure(
        t == 0 && canceled,
        format!("a 0.3 s press of a 0.5 s hold: {t} triggers, canceled {canceled}"),
    )?;
    let (t, _, completed) = hold_for(&mut rt, pad, a, 0.8);
    ensure(
        t == 1 && completed,
        format!("a 0.8 s hold: {t} triggers, completed {completed}"),
    )?;
    Ok(())
}

#[test]
fn hold_fires_once_at_its_time_and_a_short_press_cancels() {
    hold_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_a_hold_that_ignores_its_time_fails() {
    assert!(
        hold_check(InputFaults {
            hold_ignores_time: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

fn tap_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("dodge", ActionKind::Button)
                .bind(bd(pad("South")))
                .trigger(Trigger::Tap { max: 0.2 }),
        ],
    );
    let a = rt.handle("g/dodge").unwrap();
    let (t, _, _) = hold_for(&mut rt, pad, a, 0.1);
    ensure(t == 1, format!("a 0.1 s tap fired {t} times"))?;
    let (t, canceled, _) = hold_for(&mut rt, pad, a, 0.5);
    ensure(
        t == 0 && canceled,
        format!("a 0.5 s press of a 0.2 s tap fired {t} times (canceled {canceled})"),
    )?;
    Ok(())
}

#[test]
fn tap_fires_on_a_quick_release_only() {
    tap_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_a_tap_that_ignores_its_limit_fails() {
    assert!(
        tap_check(InputFaults {
            tap_ignores_max: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

fn multitap_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("dash", ActionKind::Button)
                .bind(bd(pad("East")))
                .trigger(Trigger::MultiTap { count: 2, gap: 0.3 }),
        ],
    );
    let a = rt.handle("g/dash").unwrap();
    let tap = |rt: &mut InputRuntime| {
        rt.send(pad, "East", [1.0, 0.0]);
        rt.update(DT);
        let on_press = rt.action(P0, a).triggered;
        frames(rt, 3);
        rt.send(pad, "East", [0.0, 0.0]);
        rt.update(DT);
        on_press
    };
    // Two taps 0.2 s apart: fires on the second press.
    let first = tap(&mut rt);
    frames(&mut rt, 6);
    let second = tap(&mut rt);
    ensure(
        !first && second,
        format!("double tap: first {first}, second {second}"),
    )?;
    frames(&mut rt, 30);
    // Two taps 0.6 s apart are two single taps.
    tap(&mut rt);
    frames(&mut rt, 36);
    let late = tap(&mut rt);
    ensure(!late, "taps 0.6 s apart made a double tap")?;
    Ok(())
}

#[test]
fn multi_tap_fires_on_the_completing_press_within_the_gap() {
    multitap_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_a_multi_tap_that_ignores_the_gap_fails() {
    assert!(
        multitap_check(InputFaults {
            multitap_ignores_gap: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

#[test]
fn pulse_fires_on_the_press_then_every_interval() {
    let (mut rt, pad, _) = rig(
        InputFaults::default(),
        vec![
            ActionDef::new("fire", ActionKind::Button)
                .bind(bd(pad("RightShoulder")))
                .trigger(Trigger::Pulse { interval: 0.25 }),
        ],
    );
    let a = rt.handle("g/fire").unwrap();
    rt.send(pad, "RightShoulder", [1.0, 0.0]);
    let mut n = 0;
    for _ in 0..60 {
        rt.update(DT);
        n += u32::from(rt.action(P0, a).triggered);
    }
    // t = 0, 0.25, 0.5, 0.75 (the 60th frame is t = 59/60 s).
    assert_eq!(n, 4);
}

#[test]
fn pressed_and_released_fire_on_their_edges() {
    let (mut rt, pad, _) = rig(
        InputFaults::default(),
        vec![
            ActionDef::new("p", ActionKind::Button)
                .bind(bd(pad("North")))
                .trigger(Trigger::Pressed),
            ActionDef::new("r", ActionKind::Button)
                .bind(bd(pad("West")))
                .trigger(Trigger::Released),
        ],
    );
    let (p, r) = (rt.handle("g/p").unwrap(), rt.handle("g/r").unwrap());
    rt.send(pad, "North", [1.0, 0.0]);
    rt.send(pad, "West", [1.0, 0.0]);
    rt.update(DT);
    assert!(rt.action(P0, p).triggered && !rt.action(P0, r).triggered);
    assert_eq!(rt.action(P0, r).phase, Phase::Ongoing);
    rt.update(DT);
    assert!(!rt.action(P0, p).triggered, "Pressed fires once");
    rt.send(pad, "West", [0.0, 0.0]);
    rt.update(DT);
    assert!(rt.action(P0, r).triggered);
}

fn chord_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("modifier", ActionKind::Button).bind(bd(pad("LeftShoulder"))),
            ActionDef::new("special", ActionKind::Button)
                .bind(bd(pad("North")))
                .trigger(Trigger::Chord("g/modifier".into())),
        ],
    );
    let a = rt.handle("g/special").unwrap();
    rt.send(pad, "North", [1.0, 0.0]);
    rt.update(DT);
    ensure(
        !rt.action(P0, a).triggered,
        "the chord fired without its modifier",
    )?;
    ensure(
        rt.action(P0, a).phase == Phase::Ongoing,
        "a chord-blocked action is not Ongoing",
    )?;
    rt.send(pad, "LeftShoulder", [1.0, 0.0]);
    rt.update(DT);
    ensure(
        rt.action(P0, a).triggered,
        "the chord did not fire with its modifier held",
    )?;
    Ok(())
}

#[test]
fn a_chord_fires_only_with_its_other_action() {
    chord_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_an_unchecked_chord_fails() {
    assert!(
        chord_check(InputFaults {
            chord_ignored: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

fn combo_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("light", ActionKind::Button).bind(bd(pad("West"))),
            ActionDef::new("heavy", ActionKind::Button).bind(bd(pad("North"))),
            ActionDef::new("finisher", ActionKind::Button).trigger(Trigger::Combo(vec![
                ("g/light".into(), 0.5),
                ("g/light".into(), 0.5),
                ("g/heavy".into(), 0.5),
            ])),
        ],
    );
    let fin = rt.handle("g/finisher").unwrap();
    let press = |rt: &mut InputRuntime, b: &str| -> bool {
        rt.send(pad, b, [1.0, 0.0]);
        rt.update(DT);
        let t = rt.action(P0, fin).triggered;
        rt.send(pad, b, [0.0, 0.0]);
        frames(rt, 5);
        t
    };
    let seq = |rt: &mut InputRuntime, bs: &[&str]| {
        let mut fired = false;
        for b in bs {
            fired |= press(rt, b);
        }
        fired
    };
    ensure(
        seq(&mut rt, &["West", "West", "North"]),
        "light, light, heavy did not finish",
    )?;
    frames(&mut rt, 60);
    ensure(
        !seq(&mut rt, &["West", "North", "West"]),
        "light, heavy, light finished (out of order)",
    )?;
    frames(&mut rt, 60);
    // In time twice, then too late for the last step.
    press(&mut rt, "West");
    press(&mut rt, "West");
    frames(&mut rt, 40);
    ensure(
        !press(&mut rt, "North"),
        "a heavy 0.75 s after the lights finished",
    )?;
    Ok(())
}

#[test]
fn a_combo_fires_on_its_steps_in_order_within_their_windows() {
    combo_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_a_combo_that_ignores_order_fails() {
    assert!(
        combo_check(InputFaults {
            combo_ignores_order: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

fn same_frame_tap_check(f: InputFaults) -> Check {
    let (mut rt, pad, _) = rig(
        f,
        vec![
            ActionDef::new("jump", ActionKind::Button)
                .bind(bd(pad("South")))
                .trigger(Trigger::Pressed),
        ],
    );
    let a = rt.handle("g/jump").unwrap();
    // Press and release between two frames (a 5 ms tap at 60 Hz).
    rt.tap(pad, "South");
    rt.update(DT);
    ensure(
        rt.action(P0, a).triggered,
        "a tap inside one frame was lost",
    )?;
    rt.update(DT);
    ensure(!rt.action(P0, a).triggered, "a one-frame tap fired twice")
}

#[test]
fn a_tap_inside_one_frame_is_not_lost() {
    same_frame_tap_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_dropping_same_frame_presses_fails() {
    assert!(
        same_frame_tap_check(InputFaults {
            drop_same_frame_press: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

fn consume_check(f: InputFaults) -> Check {
    let mut rt = InputRuntime::new().with_faults(f);
    rt.set_map(
        &InputMapDef::new()
            .context(
                ContextDef::new("menu")
                    .priority(10)
                    .action(ActionDef::new("confirm", ActionKind::Button).bind(bd(pad("South")))),
            )
            .context(
                ContextDef::new("play")
                    .action(ActionDef::new("jump", ActionKind::Button).bind(bd(pad("South")))),
            ),
    );
    let p = virtual_pad(&mut rt, "p");
    let (confirm, jump) = (
        rt.handle("menu/confirm").unwrap(),
        rt.handle("play/jump").unwrap(),
    );
    rt.send(p, "South", [1.0, 0.0]);
    rt.update(DT);
    ensure(
        rt.action(P0, confirm).triggered && !rt.action(P0, jump).triggered,
        "the menu's South also jumped",
    )?;
    let menu = rt.map().context("menu").unwrap();
    rt.set_context_enabled(P0, menu, false);
    rt.update(DT);
    ensure(
        rt.action(P0, jump).triggered,
        "with the menu closed South does not jump",
    )
}

#[test]
fn a_higher_priority_context_consumes_its_controls() {
    consume_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_contexts_that_do_not_consume_fail() {
    assert!(
        consume_check(InputFaults {
            no_consumption: true,
            ..InputFaults::default()
        })
        .is_err()
    );
}

#[test]
fn modifiers_parse_from_the_authored_settings() {
    // The editor writes text; the runtime reads the same text.
    let settings = [
        ("input.map.g.name", "Gameplay"),
        ("input.map.g.action.aim.kind", "Axis2D"),
        ("input.map.g.action.aim.bind.0", "Gamepad/RightStick"),
        (
            "input.map.g.action.aim.bindmod.0",
            "Deadzone(Radial, 0.2, 1) | Curve(Power, 3)",
        ),
        ("input.map.g.action.aim.modifiers", "Scale(2, 2)"),
        ("input.map.g.action.charge.bind.0", "Keyboard/F"),
        ("input.map.g.action.charge.triggers", "Hold(0.4)"),
    ];
    let (def, problems) = InputMapDef::from_settings(settings);
    assert!(problems.is_empty(), "{problems:?}");
    let mut rt = InputRuntime::new();
    assert!(rt.set_map(&def).is_empty());
    let pad = virtual_pad(&mut rt, "p");
    rt.send(pad, "RightStick", [0.6, 0.0]);
    rt.update(DT);
    let v = rt.action(P0, rt.handle("g/aim").unwrap()).value;
    // (0.6 - 0.2) / 0.8 = 0.5, cubed 0.125, scaled 0.25.
    assert!(close(v[0], 0.25), "{v:?}");
}
