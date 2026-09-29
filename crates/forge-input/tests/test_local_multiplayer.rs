//! Guard `C-input-device-assignment` (Ch.28 §28.14, DoD M7-12): **local multiplayer pairs
//! devices with players, and each player reads only their own**, tested with virtual devices
//! (two pads, a keyboard and a mouse):
//!
//! * Auto-join: pressing a button on a free device seats the next player; a keyboard and the
//!   mouse join as one seat; a join button (`Gamepad/Start`) can be required; the player cap
//!   holds; a device never belongs to two players.
//! * Isolation: pad 1's South fires player 0's jump and nobody else's; contexts are per
//!   player (one player in a menu, the other still playing).
//! * Loss and regain: a pad that disconnects is lost by its player (an event to prompt on) and
//!   goes back to the **same** player when it reconnects, even after another pad joined.
//!
//! Positive controls (W2): a runtime that does not re-pair a reconnecting device, and one
//! whose auto-join also takes owned devices, each fail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use forge_input::{
    ActionDef, ActionKind, ContextDef, ControlRef, Device, InputFaults, InputMapDef, InputRuntime,
    JoinPolicy, PlayerEvent, PlayerId,
};

type Check = Result<(), String>;

fn map() -> InputMapDef {
    InputMapDef::new()
        .context(
            ContextDef::new("play").action(
                ActionDef::new("jump", ActionKind::Button)
                    .bind(bd(pad("South")))
                    .bind(bd(key("Space"))),
            ),
        )
        .context(
            ContextDef::new("menu")
                .priority(5)
                .action(ActionDef::new("back", ActionKind::Button).bind(bd(pad("East")))),
        )
}

fn ensure(ok: bool, why: impl Into<String>) -> Check {
    if ok { Ok(()) } else { Err(why.into()) }
}

fn assignment_check(f: InputFaults) -> Check {
    let mut rt = InputRuntime::new().with_faults(f);
    rt.set_join_policy(JoinPolicy::AutoJoin {
        max_players: 3,
        button: None,
    });
    rt.set_map(&map());
    let pad1 = virtual_pad(&mut rt, "pad1");
    let pad2 = virtual_pad(&mut rt, "pad2");
    let kb = virtual_keyboard(&mut rt);
    let mouse = virtual_mouse(&mut rt);
    let jump = rt.handle("play/jump").unwrap();
    ensure(
        rt.players().count() == 0,
        "players exist before anyone pressed a button",
    )?;
    // Pad 1 joins as player 0.
    rt.tap(pad1, "South");
    rt.update(DT);
    ensure(
        rt.player_events()
            == [PlayerEvent::Joined {
                player: PlayerId(0),
                device: pad1,
            }],
        format!("pad 1's press: {:?}", rt.player_events()),
    )?;
    frames(&mut rt, 2);
    // The keyboard joins as player 1, with the mouse.
    rt.tap(kb, "Space");
    rt.update(DT);
    ensure(
        rt.owner(kb) == Some(PlayerId(1)) && rt.owner(mouse) == Some(PlayerId(1)),
        "the keyboard and mouse are not one seat",
    )?;
    frames(&mut rt, 2);
    // Pad 1 pressing again seats nobody new (it is player 0's).
    rt.tap(pad1, "South");
    rt.update(DT);
    ensure(
        rt.players().count() == 2,
        format!("{} players after pad 1 pressed again", rt.players().count()),
    )?;
    ensure(
        rt.action(PlayerId(0), jump).triggered && !rt.action(PlayerId(1), jump).triggered,
        "pad 1's South did not jump for player 0 alone",
    )?;
    frames(&mut rt, 2);
    rt.tap(pad2, "North");
    rt.update(DT);
    ensure(
        rt.owner(pad2) == Some(PlayerId(2)),
        "pad 2 did not join as player 2",
    )?;
    // Per-player contexts: player 2 opens a menu; player 0 still plays.
    let menu = rt.map().context("menu").unwrap();
    let play = rt.map().context("play").unwrap();
    for p in [PlayerId(0), PlayerId(1), PlayerId(2)] {
        rt.set_context_enabled(p, menu, false);
    }
    rt.set_context_enabled(PlayerId(2), menu, true);
    rt.set_context_enabled(PlayerId(2), play, false);
    frames(&mut rt, 2);
    rt.send(pad2, "South", [1.0, 0.0]);
    rt.send(pad1, "South", [1.0, 0.0]);
    rt.update(DT);
    ensure(
        rt.action(PlayerId(0), jump).triggered && !rt.action(PlayerId(2), jump).triggered,
        "player 2's menu did not stop only player 2's jump",
    )?;
    rt.send(pad2, "South", [0.0, 0.0]);
    rt.send(pad1, "South", [0.0, 0.0]);
    frames(&mut rt, 2);
    // Pad 1's battery dies: player 0 loses it.
    rt.disconnect(pad1);
    ensure(
        rt.player_events().contains(&PlayerEvent::DeviceLost {
            player: PlayerId(0),
            device: pad1,
        }),
        "no DeviceLost for player 0",
    )?;
    rt.update(DT);
    // It comes back (same stable key): player 0 again, not a new player.
    let back = virtual_pad(&mut rt, "pad1");
    ensure(
        rt.owner(back) == Some(PlayerId(0)),
        format!(
            "the reconnected pad went to {:?}, not player 0",
            rt.owner(back)
        ),
    )?;
    rt.tap(back, "South");
    rt.update(DT);
    ensure(
        rt.players().count() == 3,
        "the reconnected pad seated a new player",
    )?;
    ensure(
        rt.action(PlayerId(0), jump).triggered,
        "the reconnected pad does not jump for player 0",
    )?;
    Ok(())
}

#[test]
fn devices_join_their_own_players_and_come_back_to_them() {
    assignment_check(InputFaults::default()).unwrap();
}

#[test]
fn positive_control_no_re_pairing_on_reconnect_fails() {
    let e = assignment_check(InputFaults {
        no_reconnect_pairing: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("reconnected"), "{e}");
}

#[test]
fn positive_control_joining_owned_devices_fails() {
    let e = assignment_check(InputFaults {
        join_assigned_devices: true,
        ..InputFaults::default()
    })
    .unwrap_err();
    assert!(e.contains("pressed again"), "{e}");
}

#[test]
fn a_join_button_can_be_required_and_the_cap_holds() {
    let mut rt = InputRuntime::new();
    rt.set_join_policy(JoinPolicy::AutoJoin {
        max_players: 1,
        button: Some(ControlRef::new(Device::Gamepad, "Start")),
    });
    rt.set_map(&map());
    let a = virtual_pad(&mut rt, "a");
    let b = virtual_pad(&mut rt, "b");
    rt.tap(a, "South");
    rt.update(DT);
    assert_eq!(rt.players().count(), 0, "South joined without Start");
    rt.tap(a, "Start");
    rt.update(DT);
    assert_eq!(rt.owner(a), Some(PlayerId(0)));
    rt.tap(b, "Start");
    rt.update(DT);
    assert_eq!(rt.owner(b), None, "the cap of one player did not hold");
}

#[test]
fn single_player_owns_every_device_and_manual_assignment_moves_one() {
    let mut rt = InputRuntime::new();
    rt.set_map(&map());
    let a = virtual_pad(&mut rt, "a");
    let kb = virtual_keyboard(&mut rt);
    assert_eq!(
        (rt.owner(a), rt.owner(kb)),
        (Some(PlayerId(0)), Some(PlayerId(0)))
    );
    rt.set_join_policy(JoinPolicy::Manual);
    let p1 = rt.add_player().unwrap();
    rt.assign(a, p1);
    assert_eq!(rt.player_devices(PlayerId(0)), &[kb]);
    assert_eq!(rt.player_devices(p1), &[a]);
    rt.remove_player(p1);
    assert_eq!(rt.owner(a), None);
}
