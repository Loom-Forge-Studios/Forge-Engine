//! Guard `C-phys-play-in-editor` (WP-60, M4-3): **the editor's Play mode runs physics,
//! through the physics backends its plugin load registered.**
//!
//! A 3D-preset shell, headless (`forge_editor::testing::Rig`): a floor and a crate with
//! `physics.*` properties are made by bus commands, like any edit (I7), and Play is pressed on
//! the play controls. The crate falls and comes to rest on the floor in the play core's
//! transforms (what the viewport draws), for the project's `physics.backend` — avian3d (the
//! 3D preset's default) and rapier3d set by a `SetSetting` command — and Stop leaves the
//! project exactly as it was.
//!
//! The play core forks physics with the registry the shell's load filled
//! (`forge.phys.backend`), not a private copy of the first-party backends: a test plugin that
//! chains the `avian3d` backend (I16: add / replace / remove / chain are ordinary operations)
//! sees Play build its physics world through it.
//!
//! Positive control (W2): `positive_control_a_play_core_without_the_registry_bypasses_plugins`
//! — a play core given no registry (the first-party backends only) never reaches the chained
//! backend, so the check that Play went through the load's registry has teeth.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::play::{PlayBackend, PlayCommand, PlayState};
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::sim_bridge::SimPlay;
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_phys::{BackendFactory, PhysicsBackendPoint};
use forge_plugin::{InstallCx, Manifest, PluginError, SourcePlugin};
use forge_trace::Tracer;

/// Chains `forge.phys.backend/avian3d` to count the physics worlds built through it (its own
/// counter: tests running side by side cannot see each other's).
struct Counting(Manifest, Arc<AtomicU64>);

impl SourcePlugin for Counting {
    fn manifest(&self) -> &Manifest {
        &self.0
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        let built = self.1.clone();
        cx.chain::<PhysicsBackendPoint>("avian3d", move |inner: BackendFactory| {
            Arc::new(
                move |frame: forge_frames::FrameId, s: &forge_phys::PhysicsSettings| {
                    built.fetch_add(1, Ordering::SeqCst);
                    inner(frame, s)
                },
            ) as BackendFactory
        })
    }
}

fn counting() -> Counting {
    Counting(
        Manifest::parse(
            r#"Plugin(id: "com.test.countphys", version: "0.1.0", engine: "^0.1", kind: Source,
                chains: [ PhysicsBackend("avian3d") ])"#,
        )
        .unwrap(),
        Arc::new(AtomicU64::new(0)),
    )
}

/// The scene as bus commands: entity 0 a static 20 x 1 x 20 floor with its top at y = 0,
/// entity 1 a 1 m dynamic crate at y = 3.
fn scene() -> Vec<EditorCommand> {
    let set = |e: u64, path: &str, value: Value| EditorCommand::SetProperty {
        entity: EntityKey(e),
        path: path.into(),
        value,
    };
    vec![
        EditorCommand::Spawn {
            name: "Floor".into(),
            parent: None,
        },
        set(0, "transform.position.local", Value::Vec3([0.0, -0.5, 0.0])),
        set(0, "physics.body", Value::Text("static".into())),
        set(0, "physics.size", Value::Vec3([20.0, 1.0, 20.0])),
        EditorCommand::Spawn {
            name: "Crate".into(),
            parent: None,
        },
        set(1, "transform.position.local", Value::Vec3([0.0, 3.0, 0.0])),
        set(1, "physics.body", Value::Text("dynamic".into())),
        set(1, "physics.shape", Value::Text("box".into())),
    ]
}

/// Play the scene for three seconds of editor clock on `backend` (`None`: the project's
/// default). Returns the crate's drawn height and the chained backend's build count.
fn play(backend: Option<&str>, registry: bool) -> (f64, u64) {
    let tracer: &'static Tracer = Box::leak(Box::new(Tracer::new()));
    let stand_in = StandInPanels::without(&[]).unwrap();
    let plugin = counting();
    let mut cfg = assemble(
        builtin_preset("3d").unwrap(),
        &[&stand_in, &plugin],
        &[],
        None,
    )
    .unwrap();
    if !registry {
        cfg.services =
            std::mem::take(&mut cfg.services).with_play_core(SimPlay::with_tracer(tracer));
    }
    let mut rig = Rig::new(cfg).unwrap();
    let em = rig.shell.emitter().clone();
    let mut cmds = scene();
    if let Some(b) = backend {
        cmds.push(EditorCommand::SetSetting {
            key: "physics.backend".into(),
            value: Some(Value::Text(b.into())),
        });
    }
    for c in cmds {
        em.emit(c);
        rig.turn();
    }
    rig.settle();
    rig.shell.play(PlayCommand::Play);
    let services = rig.shell.handles().services_rc();
    let core = services.play_core.clone().unwrap();
    assert_eq!(core.borrow().state(), PlayState::Playing);
    for _ in 0..60 {
        rig.h.set_now(rig.h.now() + Duration::from_millis(50));
        rig.turn();
    }
    rig.settle();
    assert_eq!(core.borrow_mut().take_error(), None);
    let y = core
        .borrow()
        .transforms()
        .get(&EntityKey(1))
        .map(|t| t.0.local.y)
        .expect("the crate moves while playing");
    let built = plugin.1.load(Ordering::SeqCst);
    rig.shell.play(PlayCommand::Stop);
    rig.settle();
    (y, built)
}

#[test]
fn play_drops_the_crate_onto_the_floor_on_every_backend() {
    for b in [None, Some("avian3d"), Some("rapier3d")] {
        let (y, _) = play(b, true);
        assert!(
            (y - 0.5).abs() < 0.03,
            "{b:?}: the crate rests at {y}, not 0.5"
        );
    }
}

#[test]
fn play_forks_physics_through_the_plugin_registry() {
    let (_, built) = play(Some("avian3d"), true);
    assert!(
        built >= 1,
        "Play never built its physics through the loaded registry"
    );
}

#[test]
fn positive_control_a_play_core_without_the_registry_bypasses_plugins() {
    let (y, built) = play(Some("avian3d"), false);
    assert!((y - 0.5).abs() < 0.03, "the control still simulates: {y}");
    assert_eq!(
        built, 0,
        "a play core without the registry reached the chained backend"
    );
}
