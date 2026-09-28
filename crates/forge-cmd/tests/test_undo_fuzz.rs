//! `test_undo_fuzz` (Ch.7.3): random command sequences — single commands, gestures that
//! merge, cancels, undos, redos, selective undos, invalid commands, panicking handlers —
//! then undo everything, and the project state hash must equal the starting hash; redo
//! everything, and it must equal the end hash.
//!
//! Positive control (W2): `positive_control_mutate_undo_build_fails` rebuilds this file with
//! `--features mutate-undo` (transaction merging keeps the newest `before`, the classic
//! drag-merge bug) and asserts the fuzz FAILS there.

use std::path::PathBuf;

use forge_cmd::{
    Bus, Change, CmdError, CommandPolicy, CommandSink, DiffBuilder, EditorCommand, EntityKey,
    Issuer, TxnId, TxnState, Value,
};
use proptest::prelude::*;

const VIOLATION: &str = "UNDO DID NOT RESTORE";

/// A command, with entity choices as indices into the live entity list (resolved at run
/// time, so most commands are valid) or, rarely, a key that does not exist.
#[derive(Clone, Debug)]
enum Cmd {
    Spawn {
        parent: Option<u8>,
        name: u8,
    },
    Despawn(u8),
    Rename(u8, u8),
    Reparent(u8, Option<u8>),
    Set(u8, u8, i8),
    SetF(u8, u8, i8),
    Remove(u8, u8),
    Setting(u8, Option<i8>),
    /// Composite handler: spawn a child and tag it.
    InvokeSpawnTagged(u8),
    /// A handler that panics after describing a change.
    InvokePanic,
    /// A key that does not exist.
    Ghost,
}

#[derive(Clone, Debug)]
enum Op {
    Single(Cmd, bool),
    /// A gesture: frames of property sets (and maybe a rename) on one entity; commit or
    /// cancel.
    Gesture {
        entity: u8,
        path: u8,
        frames: Vec<i8>,
        rename: bool,
        commit: bool,
    },
    Undo,
    Redo,
    /// Selective undo of the `n`-th history entry from the end.
    UndoNth(u8),
}

fn cmd_strategy() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        3 => (proptest::option::of(any::<u8>()), any::<u8>()).prop_map(|(parent, name)| Cmd::Spawn { parent, name }),
        1 => any::<u8>().prop_map(Cmd::Despawn),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(e, n)| Cmd::Rename(e, n)),
        1 => (any::<u8>(), proptest::option::of(any::<u8>())).prop_map(|(e, p)| Cmd::Reparent(e, p)),
        3 => (any::<u8>(), 0u8..4, any::<i8>()).prop_map(|(e, p, v)| Cmd::Set(e, p, v)),
        1 => (any::<u8>(), 0u8..4, any::<i8>()).prop_map(|(e, p, v)| Cmd::SetF(e, p, v)),
        1 => (any::<u8>(), 0u8..4).prop_map(|(e, p)| Cmd::Remove(e, p)),
        1 => (0u8..3, proptest::option::of(any::<i8>())).prop_map(|(k, v)| Cmd::Setting(k, v)),
        1 => any::<u8>().prop_map(Cmd::InvokeSpawnTagged),
        1 => Just(Cmd::InvokePanic),
        1 => Just(Cmd::Ghost),
    ]
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => (cmd_strategy(), any::<bool>()).prop_map(|(c, auto)| Op::Single(c, auto)),
        2 => (any::<u8>(), 0u8..4, proptest::collection::vec(any::<i8>(), 1..8), any::<bool>(), prop::bool::weighted(0.8))
            .prop_map(|(entity, path, frames, rename, commit)| Op::Gesture { entity, path, frames, rename, commit }),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
        1 => any::<u8>().prop_map(Op::UndoNth),
    ]
}

const PATHS: [&str; 4] = ["x", "transform.scale", "hp", "tag"];
const SETTINGS: [&str; 3] = ["render.exposure", "physics.gravity", "name"];

fn pick(bus: &Bus, i: u8) -> EntityKey {
    let keys: Vec<EntityKey> = bus.project().entities().map(|(k, _)| k).collect();
    if keys.is_empty() {
        EntityKey(u64::from(i) + 10_000)
    } else {
        keys[usize::from(i) % keys.len()]
    }
}

fn to_editor(bus: &Bus, c: &Cmd) -> EditorCommand {
    match c {
        Cmd::Spawn { parent, name } => EditorCommand::Spawn {
            name: format!("n{name}"),
            parent: parent.map(|p| pick(bus, p)),
        },
        Cmd::Despawn(e) => EditorCommand::Despawn {
            entity: pick(bus, *e),
        },
        Cmd::Rename(e, n) => EditorCommand::Rename {
            entity: pick(bus, *e),
            name: format!("r{n}"),
        },
        Cmd::Reparent(e, p) => EditorCommand::Reparent {
            entity: pick(bus, *e),
            parent: p.map(|p| pick(bus, p)),
        },
        Cmd::Set(e, p, v) => EditorCommand::SetProperty {
            entity: pick(bus, *e),
            path: PATHS[usize::from(*p)].into(),
            value: Value::Int(i64::from(*v)),
        },
        Cmd::SetF(e, p, v) => EditorCommand::SetProperty {
            entity: pick(bus, *e),
            path: PATHS[usize::from(*p)].into(),
            // -0.0 and 0.0 are distinct values: undo must restore the exact bits.
            value: Value::Float(if *v == 0 { -0.0 } else { f64::from(*v) / 4.0 }),
        },
        Cmd::Remove(e, p) => EditorCommand::RemoveProperty {
            entity: pick(bus, *e),
            path: PATHS[usize::from(*p)].into(),
        },
        Cmd::Setting(k, v) => EditorCommand::SetSetting {
            key: SETTINGS[usize::from(*k)].into(),
            value: v.map(|v| Value::Int(i64::from(v))),
        },
        Cmd::InvokeSpawnTagged(p) => EditorCommand::Invoke {
            target: "fuzz.spawn_tagged".into(),
            args: format!(r#"{{"parent": {}}}"#, pick(bus, *p).0),
        },
        Cmd::InvokePanic => EditorCommand::Invoke {
            target: "fuzz.panic".into(),
            args: "{}".into(),
        },
        Cmd::Ghost => EditorCommand::SetProperty {
            entity: EntityKey(u64::MAX - 1),
            path: "x".into(),
            value: Value::Int(1),
        },
    }
}

fn new_bus() -> Bus {
    let mut bus = Bus::new();
    bus.register_handler(
        "fuzz.spawn_tagged",
        CommandPolicy::ORDINARY,
        |b: &mut DiffBuilder<'_>, args: &serde_json::Value| -> Result<(), CmdError> {
            let parent = args["parent"].as_u64().map(EntityKey);
            let parent = parent.filter(|p| b.exists(*p));
            let kid = b.spawn("tagged", parent)?;
            b.set_property(kid, "tag", Value::Text("spawned".into()))?;
            b.set_property(kid, "tag", Value::Text("final".into()))?;
            Ok(())
        },
    )
    .expect("registers");
    bus.register_handler(
        "fuzz.panic",
        CommandPolicy::ORDINARY,
        |b: &mut DiffBuilder<'_>, _: &serde_json::Value| -> Result<(), CmdError> {
            b.set_setting("half", Some(Value::Int(1)))?;
            panic!("fuzz handler panics mid-plan");
        },
    )
    .expect("registers");
    bus
}

fn issuer(auto: bool) -> Issuer {
    if auto {
        Issuer::Automation {
            session: "fuzz".into(),
            tool: "apply".into(),
        }
    } else {
        Issuer::Human {
            user: "fuzz".into(),
        }
    }
}

/// Run `ops`; return (start hash, end hash, the bus).
fn run(ops: &[Op]) -> (u64, u64, Bus) {
    let mut bus = new_bus();
    // A starting project with some content, built through the bus, history cleared.
    for i in 0..4u8 {
        let c = to_editor(
            &bus,
            &Cmd::Spawn {
                parent: if i == 0 { None } else { Some(i) },
                name: i,
            },
        );
        let e = bus.envelope(issuer(false), c);
        bus.apply(e).expect("seed");
        let c = to_editor(&bus, &Cmd::Set(i, i % 4, i as i8));
        let e = bus.envelope(issuer(false), c);
        bus.apply(e).expect("seed prop");
    }
    bus.clear_history();
    let h0 = bus.project().state_hash();

    for op in ops {
        match op {
            Op::Single(c, auto) => {
                let before = bus.project().state_hash();
                let cmd = to_editor(&bus, c);
                let e = bus.envelope(issuer(*auto), cmd);
                if bus.apply(e).is_err() {
                    assert_eq!(
                        bus.project().state_hash(),
                        before,
                        "a refusal changed state"
                    );
                }
            }
            Op::Gesture {
                entity,
                path,
                frames,
                rename,
                commit,
            } => {
                let before = bus.project().state_hash();
                let who = issuer(false);
                let t = bus.begin("gesture", who.clone());
                let target = pick(&bus, *entity);
                for (i, v) in frames.iter().enumerate() {
                    let cmd = EditorCommand::SetProperty {
                        entity: target,
                        path: PATHS[usize::from(*path)].into(),
                        value: Value::Int(i64::from(*v)),
                    };
                    let e = bus.envelope_in(t, who.clone(), cmd);
                    let _ = bus.apply(e);
                    if *rename {
                        let e = bus.envelope_in(
                            t,
                            who.clone(),
                            EditorCommand::Rename {
                                entity: target,
                                name: format!("g{i}"),
                            },
                        );
                        let _ = bus.apply(e);
                    }
                }
                if *commit {
                    bus.commit(t).expect("commit an open gesture");
                } else {
                    bus.cancel(t).expect("cancel an open gesture");
                    assert_eq!(bus.project().state_hash(), before, "cancel did not revert");
                }
            }
            Op::Undo => {
                if let Some(t) = bus.undo_target() {
                    bus.undo(t)
                        .expect("Ctrl+Z on the newest transaction always applies");
                }
            }
            Op::Redo => {
                if let Some(t) = bus.redo_target() {
                    bus.redo(t).expect("Ctrl+Y right after undo always applies");
                }
            }
            Op::UndoNth(n) => {
                let hist: Vec<TxnId> = bus
                    .history()
                    .filter(|r| r.state() == TxnState::Committed)
                    .map(|r| r.txn())
                    .collect();
                if !hist.is_empty() {
                    let t = hist[hist.len() - 1 - usize::from(*n) % hist.len()];
                    let before = bus.project().state_hash();
                    if bus.undo(t).is_err() {
                        assert_eq!(
                            bus.project().state_hash(),
                            before,
                            "a refused undo changed state"
                        );
                    }
                }
            }
        }
    }
    let h_end = bus.project().state_hash();
    (h0, h_end, bus)
}

fn undo_all_redo_all(ops: &[Op]) -> Result<(), String> {
    let (h0, h_end, mut bus) = run(ops);
    // Undo every committed transaction, newest first.
    let committed: Vec<TxnId> = bus
        .history()
        .filter(|r| r.state() == TxnState::Committed)
        .map(|r| r.txn())
        .collect();
    for t in committed.iter().rev() {
        bus.undo(*t)
            .map_err(|r| format!("{VIOLATION}: undo of {t} refused: {r}"))?;
    }
    if bus.project().state_hash() != h0 {
        return Err(format!(
            "{VIOLATION}: after undoing all {} transactions the state differs from the start",
            committed.len()
        ));
    }
    for t in &committed {
        bus.redo(*t)
            .map_err(|r| format!("{VIOLATION}: redo of {t} refused: {r}"))?;
    }
    if bus.project().state_hash() != h_end {
        return Err(format!(
            "{VIOLATION}: redoing everything did not reproduce the end state"
        ));
    }
    Ok(())
}

proptest! {
    // Fixed seed (same cases on every machine and run) and no regression file (the mutant
    // child must not write into the tree).
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x0DD0_0F0E_5EED),
        ..ProptestConfig::default()
    })]

    #[test]
    fn undo_everything_restores_the_start_and_redo_everything_restores_the_end(
        ops in proptest::collection::vec(op_strategy(), 1..48)
    ) {
        if let Err(e) = undo_all_redo_all(&ops) {
            return Err(TestCaseError::fail(e));
        }
    }
}

/// A deterministic case that exercises the merge path the mutant breaks, so the control
/// does not depend on the random draw alone.
#[test]
fn a_committed_drag_undoes_to_its_start() {
    let ops = vec![Op::Gesture {
        entity: 0,
        path: 0,
        frames: vec![5, 6, 7],
        rename: true,
        commit: true,
    }];
    if let Err(e) = undo_all_redo_all(&ops) {
        panic!("{e}");
    }
}

#[test]
fn the_fuzz_is_not_vacuous() {
    // Sequences really change state and really leave undoable history.
    let ops: Vec<Op> = (0..20)
        .map(|i| Op::Single(Cmd::Set(i, i % 4, i as i8 + 1), false))
        .collect();
    let (h0, h_end, bus) = run(&ops);
    assert_ne!(h0, h_end);
    assert!(bus.history().count() >= 10);
    assert!(
        bus.history()
            .flat_map(|r| r.changes())
            .all(|c| matches!(c, Change::Property { .. }))
    );
}

#[test]
fn positive_control_mutate_undo_build_fails() {
    if forge_cmd::MUTATE_UNDO {
        return; // we ARE the mutated child; never recurse
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../target"))
        .join("mutate-undo");
    let out = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "test",
            "--locked",
            "-p",
            "forge-cmd",
            "--features",
            "mutate-undo",
            "--test",
            "test_undo_fuzz",
            "--",
            "--test-threads=1",
            "undo_everything_restores",
            "a_committed_drag_undoes_to_its_start",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("PROPTEST_CASES")
        .current_dir(&root)
        .output()
        .expect("cannot run cargo for the mutate-undo control");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "the mutated build PASSED the undo fuzz — the guard is vacuous:\n{text}"
    );
    assert!(
        text.contains(
            "test undo_everything_restores_the_start_and_redo_everything_restores_the_end ... FAILED"
        ),
        "the property test did not fail in the mutant (did it build?):\n{text}"
    );
    assert!(
        text.contains("test a_committed_drag_undoes_to_its_start ... FAILED"),
        "the deterministic drag case did not fail in the mutant:\n{text}"
    );
    assert!(
        text.contains(VIOLATION),
        "the mutant failed, but not on an undo violation:\n{text}"
    );
}
