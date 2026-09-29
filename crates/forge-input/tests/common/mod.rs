//! Shared fixtures for forge-input's guards: virtual devices and the reference game map.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use forge_input::{
    ActionDef, ActionKind, Binding, BindingDef, ContextDef, ControlRef, DeadzoneKind, Device,
    DeviceDesc, DeviceId, InputFaults, InputMapDef, InputRuntime, Modifier, PadFamily, Trigger,
};

pub const DT: f32 = 1.0 / 60.0;

pub fn c(d: Device, n: &str) -> ControlRef {
    ControlRef::new(d, n)
}
pub fn key(n: &str) -> Binding {
    Binding::Control(c(Device::Keyboard, n))
}
pub fn pad(n: &str) -> Binding {
    Binding::Control(c(Device::Gamepad, n))
}
pub fn bd(b: Binding) -> BindingDef {
    BindingDef::new(b)
}

pub fn virtual_pad(rt: &mut InputRuntime, key: &str) -> DeviceId {
    rt.connect(
        DeviceDesc::new(Device::Gamepad, &format!("Virtual pad {key}"), key)
            .family(PadFamily::Xbox)
            .virtual_device(),
    )
}
pub fn virtual_keyboard(rt: &mut InputRuntime) -> DeviceId {
    rt.connect(DeviceDesc::new(Device::Keyboard, "Virtual keyboard", "vkb").virtual_device())
}
pub fn virtual_mouse(rt: &mut InputRuntime) -> DeviceId {
    rt.connect(DeviceDesc::new(Device::Mouse, "Virtual mouse", "vmouse").virtual_device())
}

/// Run `n` frames.
pub fn frames(rt: &mut InputRuntime, n: usize) {
    for _ in 0..n {
        rt.update(DT);
    }
}

pub fn radial(lower: f32, upper: f32) -> Modifier {
    Modifier::Deadzone {
        kind: DeadzoneKind::Radial,
        lower,
        upper,
    }
}

/// The reference game (the `input.update` budget's workload): three contexts of 16 actions
/// each, 1.5 bindings per action, stick modifiers, and every trigger kind.
pub fn reference_map() -> InputMapDef {
    let mut m = InputMapDef::new();
    let buttons = [
        "South",
        "East",
        "West",
        "North",
        "LeftShoulder",
        "RightShoulder",
        "Select",
        "Start",
        "DPadUp",
        "DPadDown",
        "DPadLeft",
        "DPadRight",
    ];
    let keys = [
        "Space", "E", "Q", "R", "F", "G", "Tab", "Escape", "Digit1", "Digit2", "Digit3", "Digit4",
    ];
    for (ci, ctx) in ["gameplay", "vehicle", "menu"].iter().enumerate() {
        let mut cx = ContextDef::new(ctx).priority(i32::try_from(ci).unwrap_or(0));
        cx = cx.action(
            ActionDef::new("move", ActionKind::Axis2D)
                .bind(
                    bd(pad("LeftStick"))
                        .with(radial(0.15, 0.95))
                        .with(Modifier::Curve(forge_input::Curve::Power(2.0))),
                )
                .bind(bd(Binding::Composite2D([
                    c(Device::Keyboard, "W"),
                    c(Device::Keyboard, "S"),
                    c(Device::Keyboard, "A"),
                    c(Device::Keyboard, "D"),
                ]))),
        );
        cx = cx.action(
            ActionDef::new("look", ActionKind::Axis2D)
                .bind(bd(pad("RightStick")).with(radial(0.1, 1.0)))
                .bind(
                    bd(Binding::Control(c(Device::Mouse, "Delta"))).with(Modifier::Scale(0.1, 0.1)),
                ),
        );
        cx = cx.action(ActionDef::new("throttle", ActionKind::Axis1D).bind(
            bd(pad("RightTrigger")).with(Modifier::Deadzone {
                kind: DeadzoneKind::Axial,
                lower: 0.05,
                upper: 1.0,
            }),
        ));
        let triggers: [Option<Trigger>; 13] = [
            None,
            Some(Trigger::Pressed),
            Some(Trigger::Released),
            Some(Trigger::Hold {
                time: 0.5,
                repeat: false,
            }),
            Some(Trigger::Tap { max: 0.2 }),
            Some(Trigger::MultiTap { count: 2, gap: 0.3 }),
            Some(Trigger::Pulse { interval: 0.25 }),
            Some(Trigger::Chord(format!("{ctx}/b0"))),
            None,
            Some(Trigger::Hold {
                time: 0.3,
                repeat: true,
            }),
            Some(Trigger::Combo(vec![
                (format!("{ctx}/b1"), 0.5),
                (format!("{ctx}/b2"), 0.5),
            ])),
            Some(Trigger::Tap { max: 0.3 }),
            Some(Trigger::Pressed),
        ];
        for (i, t) in triggers.iter().enumerate() {
            let mut a = ActionDef::new(&format!("b{i}"), ActionKind::Button)
                .bind(bd(pad(buttons[i % buttons.len()])));
            if i % 2 == 0 {
                a = a.bind(bd(key(keys[i % keys.len()])));
            }
            if let Some(t) = t {
                a = a.trigger(t.clone());
            }
            cx = cx.action(a);
        }
        m = m.context(cx);
    }
    m
}

/// A runtime playing the reference game: 4 players (keyboard + mouse, and three pads), two
/// spare pads, every context enabled.
pub fn reference_runtime(faults: InputFaults) -> (InputRuntime, Vec<DeviceId>) {
    let mut rt = InputRuntime::new().with_faults(faults);
    rt.set_join_policy(forge_input::JoinPolicy::Manual);
    let problems = rt.set_map(&reference_map());
    assert!(problems.is_empty(), "{problems:?}");
    let kb = virtual_keyboard(&mut rt);
    let mouse = virtual_mouse(&mut rt);
    let mut devs = vec![kb, mouse];
    for i in 0..6 {
        devs.push(virtual_pad(&mut rt, &format!("pad{i}")));
    }
    let p0 = rt.add_player().unwrap();
    rt.assign(kb, p0);
    rt.assign(mouse, p0);
    for i in 0..3 {
        let p = rt.add_player().unwrap();
        rt.assign(devs[2 + i], p);
    }
    (rt, devs)
}

/// One frame's raw input for the reference game: 40 events.
pub fn reference_events(rt: &mut InputRuntime, devs: &[DeviceId], frame: u64) {
    let t = frame as f32 * 0.05;
    for (k, d) in devs.iter().enumerate().skip(2) {
        let (s, co) = (t + k as f32).sin_cos();
        rt.send(*d, "LeftStick", [s * 0.8, co * 0.8]);
        rt.send(*d, "RightStick", [co * 0.5, s * 0.5]);
        rt.send(*d, "RightTrigger", [(s * 0.5 + 0.5), 0.0]);
        let down = (frame + k as u64) % 7 < 3;
        rt.send(*d, "South", [if down { 1.0 } else { 0.0 }, 0.0]);
        rt.send(
            *d,
            "West",
            [
                if (frame / 3).is_multiple_of(2) {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ],
        );
    }
    let kb = devs[0];
    for (i, k) in ["W", "A", "Space", "E", "Q", "R", "F", "G"]
        .iter()
        .enumerate()
    {
        let down = (frame + i as u64) % 11 < 5;
        rt.send(kb, k, [if down { 1.0 } else { 0.0 }, 0.0]);
    }
    rt.send(devs[1], "Delta", [1.5, -0.5]);
    rt.send(
        devs[1],
        "Left",
        [if frame.is_multiple_of(5) { 1.0 } else { 0.0 }, 0.0],
    );
}
