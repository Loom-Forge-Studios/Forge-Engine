//! `test_viewport_physics` (WP-60, gate row `C-phys-debug-draw-viewport`): **the viewport
//! draws the physics debug drawing while playing.**
//!
//! A floor and a crate with `physics.*` properties, made by bus commands and played in the
//! viewport's shell: while playing, each cell draws both bodies' collider boxes (24 lines,
//! from the play core's physics world, `forge_phys::debug`); the toolbar's "Colliders"
//! switch turns them off and back on; Stop clears them.
//!
//! Positive control (W2): `positive_control_a_scene_without_physics_draws_no_physics_lines` —
//! the same two entities without `physics.*` properties play (the viewport shows the
//! simulation) and draw no physics lines, so the count is the physics world's and not the
//! scene's boxes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{part, rig};
use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::play::{PlayCommand, PlayState};
use forge_editor::testing::Rig;
use forge_panels_scene::viewport::ViewportCanvas;

const VP: &str = "forge.viewport";

/// Entity 0 a 20 x 1 x 20 floor with its top at y = 0, entity 1 a 1 m crate at y = 3; with
/// `physics`, a static and a dynamic body.
fn scene(rig: &mut Rig, physics: bool) {
    let set = |e: u64, path: &str, value: Value| EditorCommand::SetProperty {
        entity: EntityKey(e),
        path: path.into(),
        value,
    };
    let mut cmds = vec![
        EditorCommand::Spawn {
            name: "Floor".into(),
            parent: None,
        },
        set(0, "transform.position.local", Value::Vec3([0.0, -0.5, 0.0])),
        EditorCommand::Spawn {
            name: "Crate".into(),
            parent: None,
        },
        set(1, "transform.position.local", Value::Vec3([0.0, 3.0, 0.0])),
        set(1, "motion.velocity", Value::Vec3([0.0, -1.0, 0.0])),
    ];
    if physics {
        cmds.extend([
            set(0, "physics.body", Value::Text("static".into())),
            set(0, "physics.size", Value::Vec3([20.0, 1.0, 20.0])),
            set(1, "physics.body", Value::Text("dynamic".into())),
            set(1, "physics.shape", Value::Text("box".into())),
        ]);
    }
    let em = rig.shell.emitter().clone();
    for c in cmds {
        em.emit(c);
        rig.turn();
    }
    rig.settle();
}

/// Physics lines cell 0 drew in its last paint.
fn drawn(rig: &Rig) -> usize {
    let id = part(rig, VP, &["cells"]).child(&forge_ui::Key::Index(0));
    rig.h
        .ui
        .widget::<ViewportCanvas>(id)
        .expect("cell 0 is a viewport canvas")
        .physics_lines()
}

fn play_a_second(rig: &mut Rig) {
    rig.shell.play(PlayCommand::Play);
    let s = rig.shell.handles().services_rc();
    assert_eq!(s.play.borrow().state(), PlayState::Playing);
    for _ in 0..20 {
        rig.h.set_now(rig.h.now() + Duration::from_millis(50));
        rig.turn();
    }
    rig.settle();
}

#[test]
fn the_viewport_draws_the_colliders_while_playing() {
    let mut rig = rig(&[VP]);
    scene(&mut rig, true);
    assert_eq!(drawn(&rig), 0, "stopped: no physics world, no lines");
    play_a_second(&mut rig);
    assert_eq!(
        drawn(&rig),
        24,
        "playing: the floor's and the crate's boxes"
    );
    // The switch off, then on again.
    let switch = part(&rig, VP, &["bar", "colliders"]);
    rig.h.click(switch);
    rig.settle();
    assert_eq!(drawn(&rig), 0, "the Colliders switch is off");
    rig.h.click(switch);
    rig.settle();
    assert_eq!(drawn(&rig), 24, "the Colliders switch is on again");
    rig.shell.play(PlayCommand::Stop);
    rig.settle();
    assert_eq!(drawn(&rig), 0, "stopped again");
}

#[test]
fn positive_control_a_scene_without_physics_draws_no_physics_lines() {
    let mut rig = rig(&[VP]);
    scene(&mut rig, false);
    play_a_second(&mut rig);
    let s = rig.shell.handles().services_rc();
    assert!(
        s.play.borrow().transforms().contains_key(&EntityKey(1)),
        "the crate is simulated (it moves)"
    );
    assert_eq!(drawn(&rig), 0, "no physics body, no physics line");
}
