//! Guards (Ch.28 §28.15–§28.16, DoD M7-12), through the runtime with virtual devices:
//!
//! * `C-input-touch-gestures`: tap, double tap, long press, swipe, pinch, twist and pan
//!   become Touch controls an action binds; a drag is not a tap.
//! * `C-input-onscreen-controls`: on-screen sticks and buttons drive a virtual gamepad, so
//!   actions bound to the pad work by touch; a stick and a button at once; a finger on no
//!   control reaches the gestures.
//! * `C-input-gyro`: a resting pad's bias is calibrated away (the aim does not drift) and a
//!   real turn reads as degrees turned.
//! * `C-input-haptics`: a rumble reaches the motors, mixes with another, follows its
//!   envelope and always ends with a zero command.
//!
//! Positive controls (W2): taps that ignore finger movement, on-screen controls that do not
//! capture their finger, an uncalibrated gyro, and a
//! rumble that never stops each fail their check.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_input::{
    ActionDef, ActionKind, Binding, ContextDef, Device, DeviceDesc, InputFaults, InputMapDef,
    InputRuntime, MotionSample, OnScreenControls, PlayerId, Rumble, TouchInput, TouchPhase,
};

type Check = Result<(), String>;
const P0: PlayerId = PlayerId(0);

fn ensure(ok: bool, why: impl Into<String>) -> Check {
    if ok { Ok(()) } else { Err(why.into()) }
}

fn touch_map() -> InputMapDef {
    let t = |n: &str| Binding::Control(c(Device::Touch, n));
    let mut cx = ContextDef::new("t");
    for g in ["Tap", "DoubleTap", "LongPress", "SwipeRight", "SwipeUp"] {
        cx = cx.action(ActionDef::new(&g.to_lowercase(), ActionKind::Button).bind(bd(t(g))));
    }
    cx = cx
        .action(ActionDef::new("zoom", ActionKind::Axis1D).bind(bd(t("Pinch"))))
        .action(ActionDef::new("twist", ActionKind::Axis1D).bind(bd(t("Rotate"))))
        .action(ActionDef::new("move", ActionKind::Axis2D).bind(bd(pad("LeftStick"))))
        .action(ActionDef::new("jump", ActionKind::Button).bind(bd(pad("South"))));
    InputMapDef::new().context(cx)
}

/// Frames until any of the named actions triggers (count of triggers each).
fn run(rt: &mut InputRuntime, n: usize, watch: &[&str]) -> Vec<u32> {
    let hs: Vec<_> = watch
        .iter()
        .map(|w| rt.handle(&format!("t/{w}")).unwrap())
        .collect();
    let mut counts = vec![0; watch.len()];
    for _ in 0..n {
        rt.update(DT);
        for (k, h) in hs.iter().enumerate() {
            counts[k] += u32::from(rt.action(P0, *h).triggered);
        }
    }
    counts
}

fn gestures_check(f: InputFaults) -> Check {
    let mut rt = InputRuntime::new().with_faults(f);
    rt.set_map(&touch_map());
    let ts = rt.connect(DeviceDesc::new(Device::Touch, "Touch screen", "ts").virtual_device());
    let tp = |rt: &mut InputRuntime, id, ph, x, y| rt.touch(ts, TouchInput::new(id, ph, x, y));
    // A tap.
    tp(&mut rt, 1, TouchPhase::Started, 100.0, 100.0);
    rt.update(DT);
    tp(&mut rt, 1, TouchPhase::Ended, 102.0, 101.0);
    let n = run(&mut rt, 2, &["tap", "doubletap"]);
    ensure(n == [1, 0], format!("a tap: {n:?}"))?;
    // A second tap 0.1 s later nearby: a double tap.
    run(&mut rt, 4, &[]);
    tp(&mut rt, 2, TouchPhase::Started, 105.0, 100.0);
    rt.update(DT);
    tp(&mut rt, 2, TouchPhase::Ended, 105.0, 100.0);
    let n = run(&mut rt, 2, &["tap", "doubletap"]);
    ensure(n == [1, 1], format!("a double tap: {n:?}"))?;
    run(&mut rt, 30, &[]);
    // A slow drag is neither a tap nor a swipe.
    tp(&mut rt, 3, TouchPhase::Started, 100.0, 300.0);
    rt.update(DT);
    tp(&mut rt, 3, TouchPhase::Moved, 140.0, 300.0);
    rt.update(DT);
    tp(&mut rt, 3, TouchPhase::Ended, 140.0, 300.0);
    let n = run(&mut rt, 2, &["tap", "swiperight"]);
    ensure(
        n == [0, 0],
        format!("a 40 px drag was a tap or a swipe: {n:?}"),
    )?;
    run(&mut rt, 30, &[]);
    // A fast flick right is a swipe.
    tp(&mut rt, 4, TouchPhase::Started, 100.0, 300.0);
    rt.update(DT);
    tp(&mut rt, 4, TouchPhase::Moved, 250.0, 310.0);
    rt.update(DT);
    tp(&mut rt, 4, TouchPhase::Ended, 260.0, 310.0);
    let n = run(&mut rt, 2, &["swiperight", "swipeup", "tap"]);
    ensure(n == [1, 0, 0], format!("a flick right: {n:?}"))?;
    // A finger held still is a long press, once.
    tp(&mut rt, 5, TouchPhase::Started, 400.0, 400.0);
    let n = run(&mut rt, 45, &["longpress"]);
    tp(&mut rt, 5, TouchPhase::Ended, 400.0, 400.0);
    let after = run(&mut rt, 2, &["longpress", "tap"]);
    ensure(
        n == [1] && after == [0, 0],
        format!("a 0.75 s hold: {n:?} then {after:?}"),
    )?;
    // Two fingers spread apart and turn counter-clockwise.
    tp(&mut rt, 6, TouchPhase::Started, 400.0, 400.0);
    tp(&mut rt, 7, TouchPhase::Started, 500.0, 400.0);
    rt.update(DT);
    let (zoom, twist) = (rt.handle("t/zoom").unwrap(), rt.handle("t/twist").unwrap());
    let (mut z, mut w) = (0.0, 0.0);
    for k in 1..=10 {
        let r = 50.0 + 5.0 * k as f32;
        let a = 0.03 * k as f32;
        tp(
            &mut rt,
            6,
            TouchPhase::Moved,
            450.0 - r * a.cos(),
            400.0 + r * a.sin(),
        );
        tp(
            &mut rt,
            7,
            TouchPhase::Moved,
            450.0 + r * a.cos(),
            400.0 - r * a.sin(),
        );
        rt.update(DT);
        z += rt.action(P0, zoom).value[0];
        w += rt.action(P0, twist).value[0];
    }
    ensure(
        (z - 2.0f32.ln()).abs() < 0.02 && (w - 0.3).abs() < 0.02,
        format!("pinch sum {z} (want ln 2), twist sum {w} (want 0.3)"),
    )?;
    Ok(())
}

#[test]
fn touch_gestures_become_controls() {
    gestures_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_taps_that_ignore_movement_fail() {
    let e = gestures_check(InputFaults {
        tap_no_slop: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("drag"), "{e}");
}

fn onscreen_check(f: InputFaults) -> Check {
    let mut rt = InputRuntime::new().with_faults(f);
    rt.set_map(&touch_map());
    let ts = rt.connect(DeviceDesc::new(Device::Touch, "Touch screen", "ts").virtual_device());
    let mut os = OnScreenControls::new(&mut rt, OnScreenControls::default_layout());
    os.set_screen(1000.0, 500.0);
    assert_eq!(rt.owner(os.device()), Some(P0));
    let (mv, jump, tap) = (
        rt.handle("t/move").unwrap(),
        rt.handle("t/jump").unwrap(),
        rt.handle("t/tap").unwrap(),
    );
    let stick = os.center(0);
    let button = os.center(1);
    let feed = |rt: &mut InputRuntime, os: &mut OnScreenControls, t: TouchInput| {
        if !os.touch(rt, t) {
            rt.touch(ts, t);
        }
    };
    // The thumb lands on the floating stick and pushes up-right past its radius (70 px).
    feed(
        &mut rt,
        &mut os,
        TouchInput::new(1, TouchPhase::Started, stick[0], stick[1]),
    );
    rt.update(DT);
    feed(
        &mut rt,
        &mut os,
        TouchInput::new(1, TouchPhase::Moved, stick[0] + 100.0, stick[1] - 100.0),
    );
    // The other thumb presses South at the same time.
    feed(
        &mut rt,
        &mut os,
        TouchInput::new(2, TouchPhase::Started, button[0], button[1]),
    );
    rt.update(DT);
    let v = rt.action(P0, mv).value;
    ensure(
        (v[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3 && (v[1] - v[0]).abs() < 1e-3,
        format!("the stick read {v:?} once the thumb slid past its zone"),
    )?;
    assert!(rt.action(P0, jump).triggered);
    assert!(os.state(0).pressed && os.state(1).pressed);
    // A third finger on empty screen is a tap for the gestures.
    feed(
        &mut rt,
        &mut os,
        TouchInput::new(3, TouchPhase::Started, 500.0, 250.0),
    );
    rt.update(DT);
    feed(
        &mut rt,
        &mut os,
        TouchInput::new(3, TouchPhase::Ended, 500.0, 250.0),
    );
    rt.update(DT);
    assert!(
        rt.action(P0, tap).triggered,
        "the free finger's tap was swallowed"
    );
    // Lifting the thumbs centres the stick and releases the button.
    feed(
        &mut rt,
        &mut os,
        TouchInput::new(1, TouchPhase::Ended, 0.0, 0.0),
    );
    feed(
        &mut rt,
        &mut os,
        TouchInput::new(2, TouchPhase::Ended, 0.0, 0.0),
    );
    rt.update(DT);
    assert_eq!(rt.action(P0, mv).value, [0.0, 0.0]);
    assert!(!rt.action(P0, jump).triggered);
    Ok(())
}

#[test]
fn on_screen_controls_drive_the_pad_actions_and_let_other_fingers_through() {
    onscreen_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_on_screen_controls_without_capture_fail() {
    let e = onscreen_check(InputFaults {
        onscreen_no_capture: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("slid"), "{e}");
}

fn gyro_check(f: InputFaults) -> Check {
    let mut rt = InputRuntime::new().with_faults(f);
    rt.set_map(
        &InputMapDef::new().context(
            ContextDef::new("t")
                .action(ActionDef::new("aim", ActionKind::Axis2D).bind(bd(pad("Gyro")))),
        ),
    );
    let p = virtual_pad(&mut rt, "gyro");
    let aim = rt.handle("t/aim").unwrap();
    let sample = |yaw: f32| MotionSample {
        // A 2 deg/s bias on every axis, the pad flat (gravity along +y).
        gyro: [2.0, 2.0 + yaw, 2.0],
        accel: [0.0, 1.0, 0.0],
        dt: DT,
    };
    // Two seconds at rest (the first half-second is before calibration can start).
    for _ in 0..120 {
        rt.motion(p, sample(0.0));
        rt.update(DT);
    }
    // The next second at rest must not drift the aim.
    let mut drift = [0.0f32; 2];
    for _ in 0..60 {
        rt.motion(p, sample(0.0));
        rt.update(DT);
        let v = rt.action(P0, aim).value;
        drift[0] += v[0];
        drift[1] += v[1];
    }
    ensure(
        drift[0].abs() < 0.1 && drift[1].abs() < 0.1,
        format!("a resting pad drifted the aim {drift:?} degrees in a second"),
    )?;
    // Turning at 90 deg/s for one second reads about 90 degrees of yaw.
    let mut turned = 0.0f32;
    for _ in 0..60 {
        rt.motion(p, sample(90.0));
        rt.update(DT);
        turned += rt.action(P0, aim).value[0];
    }
    ensure(
        (turned - 90.0).abs() < 3.0,
        format!("a 90 degree turn read {turned}"),
    )?;
    Ok(())
}

#[test]
fn the_gyro_calibrates_at_rest_and_reads_a_turn() {
    gyro_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_an_uncalibrated_gyro_fails() {
    let e = gyro_check(InputFaults {
        gyro_no_calibration: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("drifted"), "{e}");
}

fn haptics_check(f: InputFaults) -> Check {
    let mut rt = InputRuntime::new().with_faults(f);
    rt.set_map(&touch_map());
    let p = virtual_pad(&mut rt, "rumble");
    ensure(rt.motor_commands().is_empty(), "commands before any rumble")?;
    rt.rumble(P0, Rumble::new(0.8, 0.2, 0.5));
    rt.update(DT);
    let c = rt.motor_commands().to_vec();
    ensure(
        c.len() == 1 && c[0].device == p && (c[0].low - 0.8).abs() < 1e-6,
        format!("the first frame's commands {c:?}"),
    )?;
    rt.update(DT);
    ensure(
        rt.motor_commands().is_empty(),
        "an unchanged level was sent again",
    )?;
    // A second, stronger high-frequency effect mixes in (the strongest per motor).
    rt.rumble_device(p, Rumble::new(0.1, 0.9, 0.1));
    rt.update(DT);
    let c = rt.motor_commands().to_vec();
    ensure(
        c.len() == 1 && (c[0].low - 0.8).abs() < 1e-6 && (c[0].high - 0.9).abs() < 1e-6,
        format!("the mix {c:?}"),
    )?;
    // Everything ends: the last command is zero, then silence.
    let mut last = None;
    for _ in 0..60 {
        rt.update(DT);
        if let Some(c) = rt.motor_commands().last() {
            last = Some(*c);
        }
    }
    ensure(
        last.is_some_and(|c| c.low == 0.0 && c.high == 0.0),
        format!("the rumble did not end with a zero command: {last:?}"),
    )?;
    rt.update(DT);
    ensure(
        rt.motor_commands().is_empty(),
        "commands after the rumble ended",
    )
}

#[test]
fn rumble_mixes_follows_its_duration_and_always_stops() {
    haptics_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_a_rumble_that_never_stops_fails() {
    let e = haptics_check(InputFaults {
        rumble_never_stops: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("did not end"), "{e}");
}

#[test]
fn an_envelope_ramps_the_motor() {
    let mut rt = InputRuntime::new();
    rt.set_map(&touch_map());
    let _p = virtual_pad(&mut rt, "rumble");
    rt.rumble(P0, Rumble::new(1.0, 1.0, 1.0).envelope(0.5, 0.0));
    let mut lv = Vec::new();
    for _ in 0..20 {
        rt.update(DT);
        if let Some(c) = rt.motor_commands().first() {
            lv.push(c.low);
        }
    }
    assert!(lv.windows(2).all(|w| w[1] > w[0]), "{lv:?}");
    assert!(lv.last().is_some_and(|l| *l < 0.7), "{lv:?}");
}
