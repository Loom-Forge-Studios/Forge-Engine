//! Guard `C-input-rebinding-persists` (Ch.28 §28.13, DoD M7-12): **a player's rebinding
//! works at once and survives a restart.**
//!
//! Interactive rebinding listens for the next control that fits the action (a stick flick
//! does not bind a button action), cancels on Escape / Select, swaps or refuses a conflict
//! (naming the other action), rebinds one part of a composite, and does not fire the action
//! with the press that completed it. The overrides are saved through the real file store to
//! a temporary directory, and a **fresh runtime** (a restarted game) loads them: the new
//! control drives the action and the old one no longer does. The file holds only what the
//! player changed.
//!
//! Positive controls (W2): `positive_control_*` — a runtime that ignores loaded overrides
//! fails the restart check; a rebinding that accepts any control shape binds a stick to a
//! button action.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_input::{
    ActionDef, ActionKind, Binding, ConflictPolicy, ContextDef, Device, FileOverrideStore,
    InputFaults, InputMapDef, InputRuntime, OverrideStore, PlayerId, RebindRequest, RebindState,
};

type Check = Result<(), String>;
const P0: PlayerId = PlayerId(0);

fn map() -> InputMapDef {
    InputMapDef::new().context(
        ContextDef::new("g")
            .action(
                ActionDef::new("jump", ActionKind::Button)
                    .bind(bd(pad("South")))
                    .bind(bd(key("Space"))),
            )
            .action(ActionDef::new("attack", ActionKind::Button).bind(bd(pad("West"))))
            .action(
                ActionDef::new("move", ActionKind::Axis2D).bind(bd(Binding::Composite2D([
                    c(Device::Keyboard, "W"),
                    c(Device::Keyboard, "S"),
                    c(Device::Keyboard, "A"),
                    c(Device::Keyboard, "D"),
                ]))),
            ),
    )
}

fn runtime(f: InputFaults) -> (InputRuntime, forge_input::DeviceId, forge_input::DeviceId) {
    let mut rt = InputRuntime::new().with_faults(f);
    assert!(rt.set_map(&map()).is_empty());
    let p = virtual_pad(&mut rt, "pad");
    let k = virtual_keyboard(&mut rt);
    (rt, p, k)
}

fn press(rt: &mut InputRuntime, d: forge_input::DeviceId, ctl: &str) {
    rt.send(d, ctl, [1.0, 0.0]);
    rt.update(DT);
    rt.send(d, ctl, [0.0, 0.0]);
    rt.update(DT);
}

fn fires(rt: &mut InputRuntime, d: forge_input::DeviceId, ctl: &str, action: &str) -> bool {
    let a = rt.handle(action).unwrap();
    rt.send(d, ctl, [1.0, 0.0]);
    rt.update(DT);
    let t = rt.action(P0, a).triggered;
    rt.send(d, ctl, [0.0, 0.0]);
    frames(rt, 2);
    t
}

fn rebind_then_restart(f: InputFaults, dir: &std::path::Path) -> Check {
    let (mut rt, pad, _) = runtime(f);
    let jump = rt.handle("g/jump").unwrap();
    rt.start_rebind(RebindRequest::new(P0, jump, 0));
    // A stick flick does not bind a button action; the North press does.
    rt.send(pad, "LeftStick", [0.9, 0.0]);
    rt.update(DT);
    if rt.rebind_state() != Some(&RebindState::Waiting) {
        return Err(format!(
            "a stick flick ended the rebinding: {:?}",
            rt.rebind_state()
        ));
    }
    rt.send(pad, "LeftStick", [0.0, 0.0]);
    rt.send(pad, "North", [1.0, 0.0]);
    rt.update(DT);
    let done = rt.take_rebind();
    if !matches!(&done, Some(RebindState::Bound { binding, .. }) if *binding == common::pad("North"))
    {
        return Err(format!("the rebinding ended {done:?}"));
    }
    if rt.action(P0, jump).triggered {
        return Err("the press that completed the rebinding also jumped".into());
    }
    rt.send(pad, "North", [0.0, 0.0]);
    frames(&mut rt, 2);
    if !fires(&mut rt, pad, "North", "g/jump") || fires(&mut rt, pad, "South", "g/jump") {
        return Err("after rebinding, North does not jump or South still does".into());
    }
    let store = FileOverrideStore::new(dir);
    rt.save_bindings(P0, &store, "player1")
        .map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(store.path("player1")).map_err(|e| e.to_string())?;
    if !text.contains("Gamepad/North") || text.contains("Keyboard/Space") || text.contains("attack")
    {
        return Err(format!("the file holds more than the change:\n{text}"));
    }
    // A restart: a fresh runtime, the same map, the saved profile.
    let (mut rt2, pad2, kb2) = runtime(f);
    if !rt2
        .load_bindings(P0, &store, "player1")
        .map_err(|e| e.to_string())?
    {
        return Err("the saved profile was not found".into());
    }
    if !fires(&mut rt2, pad2, "North", "g/jump") {
        return Err("after a restart North does not jump".into());
    }
    if fires(&mut rt2, pad2, "South", "g/jump") {
        return Err("after a restart South still jumps".into());
    }
    if !fires(&mut rt2, kb2, "Space", "g/jump") {
        return Err("the untouched Space binding was lost".into());
    }
    Ok(())
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("forge-input-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_rebinding_applies_at_once_and_survives_a_restart() {
    let d = tmp("persist");
    rebind_then_restart(InputFaults::default(), &d).unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn positive_control_a_runtime_that_ignores_loaded_overrides_fails() {
    let d = tmp("persist-control");
    let e = rebind_then_restart(
        InputFaults {
            ignore_loaded_overrides: true,
            ..InputFaults::default()
        },
        &d,
    )
    .unwrap_err();
    let _ = std::fs::remove_dir_all(&d);
    assert!(e.contains("after a restart North does not jump"), "{e}");
}

#[test]
fn positive_control_a_rebinding_that_accepts_any_shape_fails() {
    let d = tmp("persist-shape");
    let e = rebind_then_restart(
        InputFaults {
            rebind_any_shape: true,
            ..InputFaults::default()
        },
        &d,
    )
    .unwrap_err();
    let _ = std::fs::remove_dir_all(&d);
    assert!(e.contains("stick flick"), "{e}");
}

#[test]
fn conflicts_swap_or_are_refused_by_name() {
    let (mut rt, pad, _) = runtime(InputFaults::default());
    let (jump, attack) = (rt.handle("g/jump").unwrap(), rt.handle("g/attack").unwrap());
    // Refuse: West is attack's.
    let mut req = RebindRequest::new(P0, jump, 0);
    req.conflicts = ConflictPolicy::Refuse;
    rt.start_rebind(req);
    press(&mut rt, pad, "West");
    match rt.take_rebind() {
        Some(RebindState::Refused(why)) => assert!(why.contains("attack"), "{why}"),
        o => panic!("{o:?}"),
    }
    // Swap: jump takes West, attack takes jump's old South.
    rt.start_rebind(RebindRequest::new(P0, jump, 0));
    press(&mut rt, pad, "West");
    assert!(matches!(
        rt.take_rebind(),
        Some(RebindState::Bound { swapped: Some((a, 0)), .. }) if a == attack
    ));
    frames(&mut rt, 2);
    assert!(fires(&mut rt, pad, "West", "g/jump"));
    assert!(fires(&mut rt, pad, "South", "g/attack"));
    // Reset puts both back.
    rt.reset_bindings(P0, None);
    assert!(fires(&mut rt, pad, "South", "g/jump"));
    assert!(fires(&mut rt, pad, "West", "g/attack"));
}

#[test]
fn escape_cancels_and_a_timeout_ends_the_wait() {
    let (mut rt, _, kb) = runtime(InputFaults::default());
    let jump = rt.handle("g/jump").unwrap();
    rt.start_rebind(RebindRequest::new(P0, jump, 1));
    press(&mut rt, kb, "Escape");
    assert_eq!(rt.take_rebind(), Some(RebindState::Canceled));
    let mut req = RebindRequest::new(P0, jump, 1);
    req.timeout = Some(0.5);
    rt.start_rebind(req);
    frames(&mut rt, 40);
    assert_eq!(rt.take_rebind(), Some(RebindState::TimedOut));
    assert!(
        rt.overrides(P0)
            .is_some_and(forge_input::BindingOverrides::is_empty)
    );
}

#[test]
fn one_part_of_a_composite_rebinds() {
    let (mut rt, _, kb) = runtime(InputFaults::default());
    let mv = rt.handle("g/move").unwrap();
    let mut req = RebindRequest::new(P0, mv, 0);
    req.part = Some(0);
    rt.start_rebind(req);
    press(&mut rt, kb, "I");
    assert!(matches!(rt.take_rebind(), Some(RebindState::Bound { .. })));
    frames(&mut rt, 2);
    rt.send(kb, "I", [1.0, 0.0]);
    rt.update(DT);
    assert_eq!(rt.action(P0, mv).value, [0.0, 1.0]);
    rt.send(kb, "I", [0.0, 0.0]);
    rt.send(kb, "W", [1.0, 0.0]);
    rt.update(DT);
    assert_eq!(rt.action(P0, mv).value, [0.0, 0.0], "W still moves up");
}

#[test]
fn a_profile_name_is_checked_and_a_missing_file_is_the_defaults() {
    let d = tmp("names");
    let store = FileOverrideStore::new(&d);
    assert!(store.load("../evil").is_err());
    assert!(matches!(store.load("nobody"), Ok(None)));
    let _ = std::fs::remove_dir_all(&d);
}
