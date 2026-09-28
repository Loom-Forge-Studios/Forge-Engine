//! The bus's derivation hook and the entity-reference index (additive, WP-U20).
//!
//! * A [`CommandDeriver`]'s changes are part of the command's diff everywhere the diff is
//!   planned: `apply`, a dry run, a batch preview and `plan_for` agree, and one undo takes
//!   the command and what it derived back together. A refusal applies nothing.
//! * `Project::referrers` and `DiffBuilder::referrers` answer who holds a
//!   `Value::Entity(k)` from an index kept by the only mutation path, including a command's
//!   own pending changes; the answer always equals a full scan (checked after a random
//!   sequence of commands, undos and redos).

use std::sync::Arc;

use forge_cmd::{
    Bus, Change, CmdError, CommandDeriver, CommandSink, DiffBuilder, EditorCommand, EntityKey,
    Issuer, Project, Value,
};

/// Mirrors every `color` edit on an entity onto the entities whose `follows` names it.
struct Follow;

impl CommandDeriver for Follow {
    fn derive(&self, _cmd: &EditorCommand, b: &mut DiffBuilder<'_>) -> Result<(), CmdError> {
        let mut i = 0;
        while i < b.changes().len() {
            if let Change::Property {
                entity,
                path,
                after,
                ..
            } = &b.changes()[i]
                && &**path == "color"
            {
                let (entity, after) = (*entity, after.clone());
                for f in b.referrers(entity, "follows") {
                    match &after {
                        Some(v) => b.set_property(f, "color", v.clone())?,
                        None => b.remove_property(f, "color")?,
                    }
                }
            }
            if let Change::Property { path, after, .. } = &b.changes()[i]
                && &**path == "color"
                && after == &Some(Value::Text("forbidden".into()))
            {
                return Err(CmdError::PolicyRefused {
                    what: "forbidden colour".into(),
                });
            }
            i += 1;
        }
        Ok(())
    }
}

fn spawn(bus: &mut Bus, name: &str) -> EntityKey {
    let e = bus.envelope(
        Issuer::Test,
        EditorCommand::Spawn {
            name: name.into(),
            parent: None,
        },
    );
    match &bus
        .apply(e)
        .unwrap_or_else(|r| panic!("{r:?}"))
        .diff
        .changes[0]
    {
        Change::Created { entity, .. } => *entity,
        c => panic!("{c:?}"),
    }
}

fn set(bus: &mut Bus, entity: EntityKey, path: &str, value: Value) -> Result<(), CmdError> {
    let e = bus.envelope(
        Issuer::Test,
        EditorCommand::SetProperty {
            entity,
            path: path.into(),
            value,
        },
    );
    bus.apply(e).map(|_| ()).map_err(|r| r.error)
}

#[test]
fn derived_changes_are_one_diff_everywhere_and_one_undo_step() {
    let mut bus = Bus::new();
    bus.set_deriver(Some(Arc::new(Follow)));
    let lead = spawn(&mut bus, "lead");
    let a = spawn(&mut bus, "a");
    let b = spawn(&mut bus, "b");
    set(&mut bus, a, "follows", Value::Entity(lead)).unwrap_or_else(|e| panic!("{e}"));
    set(&mut bus, b, "follows", Value::Entity(lead)).unwrap_or_else(|e| panic!("{e}"));
    let before = bus.project().state_hash();
    let cmd = EditorCommand::SetProperty {
        entity: lead,
        path: "color".into(),
        value: Value::Text("red".into()),
    };
    // Dry run, plan_for and a batch preview all see the derived changes.
    let env = bus.envelope(Issuer::Test, cmd.clone());
    let dry = bus.dry_run(&env).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(dry.changes.len(), 3, "{dry:?}");
    assert_eq!(
        bus.project().state_hash(),
        before,
        "a dry run changes nothing"
    );
    let planned = bus
        .plan_for(bus.project(), &Issuer::Test, &cmd)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(planned, dry);
    let applied = bus.apply(env).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(
        applied.diff, dry,
        "apply applies exactly the dry run's diff"
    );
    for e in [a, b] {
        assert_eq!(
            bus.project().entity(e).and_then(|d| d.property("color")),
            Some(&Value::Text("red".into()))
        );
    }
    // One undo takes the edit and what it derived back.
    bus.undo(applied.txn).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(bus.project().state_hash(), before);
    // A refusal from the deriver applies nothing.
    let err = set(&mut bus, lead, "color", Value::Text("forbidden".into())).expect_err("refused");
    assert_eq!(err.code().as_str(), "CMD-0014");
    assert_eq!(bus.project().state_hash(), before);
}

#[test]
fn a_panicking_deriver_is_a_rejection_not_an_unwind() {
    struct Boom;
    impl CommandDeriver for Boom {
        fn derive(&self, _: &EditorCommand, _: &mut DiffBuilder<'_>) -> Result<(), CmdError> {
            panic!("deriver bug")
        }
    }
    let mut bus = Bus::new();
    bus.set_deriver(Some(Arc::new(Boom)));
    let e = bus.envelope(
        Issuer::Test,
        EditorCommand::Spawn {
            name: "x".into(),
            parent: None,
        },
    );
    let r = bus.apply(e).expect_err("refused");
    assert_eq!(r.error.code().as_str(), "CMD-0009");
    assert!(bus.project().is_empty());
}

/// Every `(referrer, path)` holding `Value::Entity(target)`, by scanning.
fn scan(p: &Project, target: EntityKey) -> Vec<(EntityKey, String)> {
    let mut out = Vec::new();
    for (k, e) in p.entities() {
        for (path, v) in e.properties() {
            if *v == Value::Entity(target) {
                out.push((k, path.to_string()));
            }
        }
    }
    out
}

#[test]
fn the_reference_index_always_equals_a_scan() {
    // A small deterministic LCG drives a random mix of edits, undos and redos.
    let mut s: u64 = 0x5eed;
    let mut next = |n: u64| {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (s >> 33) % n
    };
    let mut bus = Bus::new();
    let keys: Vec<EntityKey> = (0..8).map(|i| spawn(&mut bus, &format!("e{i}"))).collect();
    let paths = ["target", "base", "owner"];
    let mut txns = Vec::new();
    for _ in 0..600 {
        let e = keys[next(8) as usize];
        let path = paths[next(3) as usize];
        let cmd = match next(5) {
            0 => EditorCommand::RemoveProperty {
                entity: e,
                path: path.into(),
            },
            1 => EditorCommand::SetProperty {
                entity: e,
                path: path.into(),
                value: Value::Int(next(3) as i64),
            },
            2 if !txns.is_empty() => {
                let t = txns[next(txns.len() as u64) as usize];
                let _ = bus.undo(t);
                continue;
            }
            3 if !txns.is_empty() => {
                let t = txns[next(txns.len() as u64) as usize];
                let _ = bus.redo(t);
                continue;
            }
            _ => EditorCommand::SetProperty {
                entity: e,
                path: path.into(),
                value: Value::Entity(keys[next(8) as usize]),
            },
        };
        let env = bus.envelope(Issuer::Test, cmd);
        if let Ok(a) = bus.apply(env) {
            txns.push(a.txn);
        }
        for t in &keys {
            let mut indexed: Vec<(EntityKey, String)> = bus
                .project()
                .referrers(*t)
                .map(|(e, p)| (e, p.to_string()))
                .collect();
            indexed.sort();
            let mut scanned = scan(bus.project(), *t);
            scanned.sort();
            assert_eq!(indexed, scanned, "index drifted for {t}");
        }
    }
    // A command's own pending changes are seen by the builder's referrers.
    let p = bus.project().clone();
    let mut b = DiffBuilder::new(&p);
    let fresh = b.spawn("fresh", None).unwrap_or_else(|e| panic!("{e}"));
    b.set_property(fresh, "owner", Value::Entity(keys[0]))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut want: Vec<EntityKey> = scan(&p, keys[0])
        .into_iter()
        .filter(|(_, path)| path == "owner")
        .map(|(e, _)| e)
        .collect();
    want.push(fresh);
    want.sort();
    assert_eq!(b.referrers(keys[0], "owner"), want);
    assert!(b.roots().contains(&fresh));
}
