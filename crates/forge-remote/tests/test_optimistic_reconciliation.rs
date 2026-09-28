//! `test_optimistic_reconciliation` (O-13, Ch.21 §21.18, Ch.34 §34.2; DoD M2-16): **a
//! forced divergence converges in one frame.**
//!
//! The editor shell runs as a remote client over real QUIC, with a 300 ms simulated link so
//! the host's answer can never arrive before the client has shown its prediction. Each
//! scenario checks both halves of O-13:
//!
//! * **the prediction is shown at once** — the turn the command is emitted, the mirror
//!   already shows its dry run (the pending overlay), before any answer;
//! * **a wrong prediction is gone one frame after the answer** — once the host's answers
//!   have arrived, exactly one loop turn later the mirror equals the host's project.
//!
//! Scenarios: (a) a concurrent edit — an automation session on the host spawns first, so the
//! client's predicted spawn key belongs to the session's entity and the client's entity gets the
//! next key; (b) a refusal — the device may not make destructive edits, so its predicted despawn
//! is refused by the host's grant check and the entity comes back; (c) a 20-frame gesture predicted
//! frame by frame ends where the core ends, as one undo entry.
//!
//! Positive control (W2): `positive_control_overlays_never_dropped_fail` keeps every overlay
//! after its request settles (`MirrorFaults::keep_overlays`); scenario (b) must then fail.

mod common;

use std::time::Duration;

use common::{Host, one_frame_after_answers, remote_rig, shell_config};
use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_editor::mirror::MirrorFaults;
use forge_remote::HostFaults;

const LINK: Duration = Duration::from_millis(300);

fn names(rig: &forge_editor::testing::Rig) -> Vec<(u64, String)> {
    rig.shell
        .mirror()
        .entities()
        .map(|(k, e)| (k.0, e.name.clone()))
        .collect()
}

fn converged(rig: &forge_editor::testing::Rig) -> Result<(), String> {
    let project = EditorCore::read(&rig.core, Clone::clone);
    if rig.shell.mirror().matches(&project) {
        Ok(())
    } else {
        let host: Vec<(u64, String)> = project
            .entities()
            .map(|(k, e)| (k.0, e.name().to_string()))
            .collect();
        Err(format!(
            "NOT CONVERGED one frame after the answer: the mirror shows {:?} (settings {:?}), the host has {host:?}",
            names(rig),
            rig.shell
                .mirror()
                .settings()
                .map(|(k, _)| k.to_string())
                .collect::<Vec<_>>()
        ))
    }
}

fn check(faults: MirrorFaults) -> Result<(), String> {
    let mut host = Host::with(EditorCore::new(), HostFaults::default(), LINK);
    let (mut rig, handle, _) = remote_rig(&mut host, shell_config(), "laptop");
    rig.shell.handles().set_mirror_faults(faults);

    // (a) A concurrent edit: an automation session on the host spawns first.
    let mut auto = EditorCore::connect(
        &host.core,
        Issuer::Automation {
            session: "s1".into(),
            tool: "apply".into(),
        },
    );
    auto.apply(
        EditorCommand::Spawn {
            name: "AgentThing".into(),
            parent: None,
        },
        None,
    );
    // The client has not heard of it: it predicts key 0 for its own spawn.
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Cube".into(),
        parent: None,
    });
    rig.turn();
    if names(&rig) != [(0, "Cube".to_string())] {
        return Err(format!(
            "the prediction was not shown at once: {:?}",
            names(&rig)
        ));
    }
    if rig.shell.mirror().overlays() != 1 {
        return Err("the prediction is not a pending overlay".into());
    }
    one_frame_after_answers(&mut rig, &handle);
    converged(&rig)?;
    let want = [(0, "AgentThing".to_string()), (1, "Cube".to_string())];
    if names(&rig) != want {
        return Err(format!("{:?}", names(&rig)));
    }

    // (b) A refusal: this device may not make destructive edits.
    rig.shell.emitter().emit(EditorCommand::Despawn {
        entity: EntityKey(1),
    });
    rig.turn();
    if names(&rig).len() != 1 {
        return Err(format!(
            "the predicted despawn was not shown at once: {:?}",
            names(&rig)
        ));
    }
    one_frame_after_answers(&mut rig, &handle);
    converged(&rig)?;
    let toast = rig
        .shell
        .session()
        .notifications
        .history()
        .any(|n| n.code.as_deref() == Some("CMD-0014"));
    if !toast {
        return Err("the refusal was not shown to the person".into());
    }

    // (c) A predicted gesture ends where the core ends, as one undo entry.
    let mut g = rig.shell.emitter().gesture("Drag scale");
    for i in 0..20 {
        g.update(EditorCommand::SetProperty {
            entity: EntityKey(1),
            path: "scale".into(),
            value: Value::Float(1.0 + f64::from(i) * 0.25),
        });
        rig.turn();
        let shown = rig.shell.mirror().property(EntityKey(1), "scale").cloned();
        if shown != Some(Value::Float(1.0 + f64::from(i) * 0.25)) {
            return Err(format!(
                "frame {i} of the drag was not shown at once: {shown:?}"
            ));
        }
    }
    g.commit();
    rig.turn();
    one_frame_after_answers(&mut rig, &handle);
    converged(&rig)?;
    let drags = rig
        .shell
        .mirror()
        .history()
        .iter()
        .filter(|h| h.label == "Drag scale")
        .count();
    if drags != 1 {
        return Err(format!("the drag is {drags} undo entries"));
    }
    if rig.shell.mirror().overlays() != 0 {
        return Err(format!(
            "{} overlays left after every request settled",
            rig.shell.mirror().overlays()
        ));
    }
    Ok(())
}

#[test]
fn a_forced_divergence_converges_one_frame_after_the_answer() {
    check(MirrorFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_overlays_never_dropped_fail() {
    let e = check(MirrorFaults {
        keep_overlays: true,
    })
    .expect_err("a mirror that never drops a settled overlay must fail");
    assert!(e.contains("NOT CONVERGED"), "{e}");
}
