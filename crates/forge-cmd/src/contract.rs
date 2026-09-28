//! The command contract (I8) as a reusable check: every `EditorCommand` variant is
//! dry-runnable without mutating, produces an undo entry, is undone exactly, is
//! provenance-tagged, and round-trips through serde and reflection.
//!
//! It is generic over [`ContractSubject`] so the same check runs on the in-process [`Bus`]
//! today and on `RemoteBus` when the split editor lands (M2-16), and so the guard's positive
//! controls can feed it deliberately broken sinks
//! (`tests/liveness/test_command_contract.rs`).

use std::fmt;
use std::panic::{self, AssertUnwindSafe};

use bevy_reflect::enums::Enum;
use bevy_reflect::{FromReflect, PartialReflect, TypeInfo, Typed};

use crate::{
    Bus, CommandEnvelope, CommandPolicy, CommandSink, DiffBuilder, EditorCommand, EntityKey,
    Issuer, TxnState, Value,
};

/// Something the contract can be checked on.
pub trait ContractSubject: CommandSink {
    /// Hash of the project content.
    fn state_hash(&self) -> u64;
    /// Number of transactions an undo could take back right now.
    fn undo_depth(&self) -> usize;
    /// A fresh envelope for `cmd` in its own transaction, issued by [`Issuer::Test`].
    fn envelope(&mut self, cmd: EditorCommand) -> CommandEnvelope;
    /// The envelope after a trip over this subject's wire (the command log, the remote
    /// transport). JSON by default.
    fn wire_roundtrip(&self, e: &CommandEnvelope) -> Result<CommandEnvelope, String> {
        let text = serde_json::to_string(e).map_err(|x| x.to_string())?;
        serde_json::from_str(&text).map_err(|x| x.to_string())
    }
}

impl ContractSubject for Bus {
    fn state_hash(&self) -> u64 {
        self.project().state_hash()
    }

    fn undo_depth(&self) -> usize {
        self.history()
            .filter(|r| r.state() == TxnState::Committed && r.undoable())
            .count()
    }

    fn envelope(&mut self, cmd: EditorCommand) -> CommandEnvelope {
        Bus::envelope(self, Issuer::Test, cmd)
    }
}

/// Which clause of the contract a sample broke.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    /// A variant of `EditorCommand` (per its reflected `TypeInfo`) has no sample.
    MissingSample,
    /// The sample was refused by `dry_run` or `apply` (samples must be valid, or the check is
    /// vacuous).
    SampleRejected,
    /// The sample changed nothing (vacuous: undo would trivially "restore" the state).
    SampleIsNoop,
    /// `dry_run` changed the project.
    DryRunMutated,
    /// `dry_run`'s diff is not the diff `apply` applied (the optimistic preview would lie).
    DryRunDisagreesWithApply,
    /// `apply` left no undo entry.
    NoUndoEntry,
    /// Undo was refused, or did not restore the state hash.
    UndoDidNotRestore,
    /// Redo was refused, or did not reproduce the applied state.
    RedoDidNotRestore,
    /// The `Applied` event does not carry the envelope's issuer.
    NotProvenanceTagged,
    /// The envelope does not survive the subject's wire unchanged.
    SerdeRoundTrip,
    /// The command does not survive `FromReflect` unchanged.
    ReflectRoundTrip,
    /// `dry_run` or `apply` of a panicking command unwound, accepted it, changed the state,
    /// or refused it with anything but `CMD-0009` (the contract says both are **total**: a
    /// panicking command is a `Rejection`, never an unwind, Ch.1.2).
    NotTotal,
}

/// One broken clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The variant (its reflected name).
    pub variant: String,
    /// The clause.
    pub rule: Rule,
    /// Details.
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}: {}", self.variant, self.rule, self.detail)
    }
}

/// Target of the fixture's `Invoke` handler.
pub const FIXTURE_HANDLER: &str = "fixture.tag";

/// Target of the fixture's deliberately panicking `Invoke` handler (the totality sample).
pub const FIXTURE_PANIC: &str = "fixture.panic";

/// Fixture entity keys: `World` (root) > {`Player` (hp, pos), `Camera`}, `Lights` (root).
pub const WORLD: EntityKey = EntityKey(0);
/// See [`WORLD`].
pub const PLAYER: EntityKey = EntityKey(1);
/// See [`WORLD`].
pub const CAMERA: EntityKey = EntityKey(2);
/// See [`WORLD`].
pub const LIGHTS: EntityKey = EntityKey(3);

/// The contract fixture: a small project built **through the bus**, with its history
/// cleared so the undo depth starts at zero, and a test `Invoke` handler (`fixture.tag`:
/// sets a `tag` text property; labelled as a fixture, not an engine command).
#[must_use]
pub fn fixture() -> Bus {
    let mut bus = Bus::new();
    let registered = bus.register_handler(
        FIXTURE_HANDLER,
        CommandPolicy::ORDINARY,
        |b: &mut DiffBuilder<'_>, args: &serde_json::Value| {
            let bad = |why: &str| crate::CmdError::BadArgs {
                target: FIXTURE_HANDLER.into(),
                why: why.into(),
            };
            let entity = args["entity"]
                .as_u64()
                .ok_or_else(|| bad("`entity` must be an integer key"))?;
            let tag = args["tag"]
                .as_str()
                .ok_or_else(|| bad("`tag` must be text"))?;
            b.set_property(EntityKey(entity), "tag", Value::Text(tag.to_string()))
        },
    );
    debug_assert!(registered.is_ok());
    let registered = bus.register_handler(
        FIXTURE_PANIC,
        CommandPolicy::ORDINARY,
        |b: &mut DiffBuilder<'_>, _: &serde_json::Value| -> Result<(), crate::CmdError> {
            // Describe a change, then panic halfway: a buggy plugin command.
            b.set_property(PLAYER, "half", Value::Int(1))?;
            panic!("fixture.panic: a command handler panicked mid-plan")
        },
    );
    debug_assert!(registered.is_ok());
    let steps = [
        EditorCommand::Spawn {
            name: "World".into(),
            parent: None,
        },
        EditorCommand::Spawn {
            name: "Player".into(),
            parent: Some(WORLD),
        },
        EditorCommand::Spawn {
            name: "Camera".into(),
            parent: Some(WORLD),
        },
        EditorCommand::Spawn {
            name: "Lights".into(),
            parent: None,
        },
        EditorCommand::SetProperty {
            entity: PLAYER,
            path: "hp".into(),
            value: Value::Int(10),
        },
        EditorCommand::SetProperty {
            entity: PLAYER,
            path: "transform.position".into(),
            value: Value::Vec3([1.0, 2.0, 3.0]),
        },
        EditorCommand::SetSetting {
            key: "render.exposure".into(),
            value: Some(Value::Float(1.0)),
        },
    ];
    for cmd in steps {
        let e = Bus::envelope(&mut bus, Issuer::Test, cmd);
        let ok = bus.apply(e);
        debug_assert!(ok.is_ok(), "fixture step refused: {ok:?}");
    }
    bus.clear_history();
    bus
}

/// One valid, state-changing sample per `EditorCommand` variant, against [`fixture`].
#[must_use]
pub fn samples() -> Vec<EditorCommand> {
    vec![
        EditorCommand::Spawn {
            name: "Enemy".into(),
            parent: Some(WORLD),
        },
        EditorCommand::Despawn { entity: WORLD },
        EditorCommand::Rename {
            entity: PLAYER,
            name: "Hero".into(),
        },
        EditorCommand::Reparent {
            entity: CAMERA,
            parent: Some(LIGHTS),
        },
        EditorCommand::SetProperty {
            entity: PLAYER,
            path: "hp".into(),
            value: Value::Int(5),
        },
        EditorCommand::RemoveProperty {
            entity: PLAYER,
            path: "transform.position".into(),
        },
        EditorCommand::SetSetting {
            key: "render.exposure".into(),
            value: Some(Value::Float(2.5)),
        },
        EditorCommand::Invoke {
            target: FIXTURE_HANDLER.into(),
            args: r#"{"entity": 1, "tag": "boss"}"#.into(),
        },
    ]
}

/// The variant names `EditorCommand`'s reflected `TypeInfo` declares — an independent
/// source from any hand-written list, so a new variant cannot hide from the guard.
#[must_use]
pub fn reflected_variants() -> Vec<&'static str> {
    match EditorCommand::type_info() {
        TypeInfo::Enum(info) => info.variant_names().to_vec(),
        _ => Vec::new(),
    }
}

/// Every reflected variant has a sample.
#[must_use]
pub fn check_coverage(samples: &[EditorCommand]) -> Vec<Violation> {
    let covered: Vec<&str> = samples.iter().map(Enum::variant_name).collect();
    let declared = reflected_variants();
    let mut out: Vec<Violation> = declared
        .iter()
        .filter(|v| !covered.contains(v))
        .map(|v| Violation {
            variant: (*v).to_string(),
            rule: Rule::MissingSample,
            detail: "no sample exercises this variant".into(),
        })
        .collect();
    if declared.is_empty() {
        out.push(Violation {
            variant: "EditorCommand".into(),
            rule: Rule::MissingSample,
            detail: "EditorCommand's TypeInfo is not an enum".into(),
        });
    }
    out
}

/// Check every clause for every sample, each on a fresh subject from `make`.
pub fn check_contract<S: ContractSubject>(
    make: impl Fn() -> S,
    samples: &[EditorCommand],
) -> Vec<Violation> {
    let mut out = Vec::new();
    for cmd in samples {
        let variant = Enum::variant_name(cmd).to_string();
        let mut v = |rule: Rule, detail: String| {
            out.push(Violation {
                variant: variant.clone(),
                rule,
                detail,
            });
        };

        match EditorCommand::from_reflect(cmd.as_partial_reflect()) {
            Some(back) if back == *cmd => {}
            other => v(Rule::ReflectRoundTrip, format!("came back as {other:?}")),
        }

        let mut s = make();
        let h0 = s.state_hash();
        let depth0 = s.undo_depth();
        let env = s.envelope(cmd.clone());
        match s.wire_roundtrip(&env) {
            Ok(back) if back == env => {}
            Ok(back) => v(Rule::SerdeRoundTrip, format!("came back as {back:?}")),
            Err(e) => v(Rule::SerdeRoundTrip, e),
        }

        let dry = match s.dry_run(&env) {
            Ok(d) => d,
            Err(r) => {
                v(Rule::SampleRejected, format!("dry_run refused: {r}"));
                continue;
            }
        };
        if s.state_hash() != h0 {
            v(Rule::DryRunMutated, "the state hash changed".into());
            continue;
        }
        let h_dry = s.state_hash();
        let applied = match s.apply(env.clone()) {
            Ok(a) => a,
            Err(r) => {
                v(Rule::SampleRejected, format!("apply refused: {r}"));
                continue;
            }
        };
        let h1 = s.state_hash();
        if h1 == h_dry {
            v(Rule::SampleIsNoop, "apply changed nothing".into());
            continue;
        }
        if applied.diff != dry {
            v(
                Rule::DryRunDisagreesWithApply,
                format!("dry_run {dry:?} but apply {:?}", applied.diff),
            );
        }
        if applied.issuer != env.issuer {
            v(
                Rule::NotProvenanceTagged,
                format!("sent by {}, applied as {}", env.issuer, applied.issuer),
            );
        }
        if s.undo_depth() != depth0 + 1 {
            v(
                Rule::NoUndoEntry,
                format!("undo depth {} -> {}", depth0, s.undo_depth()),
            );
        }
        match s.undo(env.txn) {
            Ok(_) if s.state_hash() == h0 => {}
            Ok(_) => v(
                Rule::UndoDidNotRestore,
                "state hash differs after undo".into(),
            ),
            Err(r) => {
                v(Rule::UndoDidNotRestore, format!("undo refused: {r}"));
                continue;
            }
        }
        match s.redo(env.txn) {
            Ok(_) if s.state_hash() == h1 => {}
            Ok(_) => v(
                Rule::RedoDidNotRestore,
                "state hash differs after redo".into(),
            ),
            Err(r) => v(Rule::RedoDidNotRestore, format!("redo refused: {r}")),
        }
    }
    out
}

/// Commands whose planning panics, against [`fixture`]: the samples of the totality clause.
#[must_use]
pub fn panicking_samples() -> Vec<EditorCommand> {
    vec![EditorCommand::Invoke {
        target: FIXTURE_PANIC.into(),
        args: "{}".into(),
    }]
}

/// The totality clause: `dry_run` and `apply` of a panicking command each return a
/// `CMD-0009` rejection — they never unwind into the caller, never accept it, and leave the
/// state exactly as it was. Each sample runs on a fresh subject from `make`.
pub fn check_totality<S: ContractSubject>(
    make: impl Fn() -> S,
    samples: &[EditorCommand],
) -> Vec<Violation> {
    let mut out = Vec::new();
    for cmd in samples {
        let variant = Enum::variant_name(cmd).to_string();
        let mut s = make();
        let h0 = s.state_hash();
        let env = s.envelope(cmd.clone());
        let dry = panic::catch_unwind(AssertUnwindSafe(|| s.dry_run(&env)))
            .map(|r| r.map(|_| ()).map_err(|e| e.code()));
        let applied = panic::catch_unwind(AssertUnwindSafe(|| s.apply(env.clone())))
            .map(|r| r.map(|_| ()).map_err(|e| e.code()));
        for (what, got) in [("dry_run", dry), ("apply", applied)] {
            let detail = match got {
                Err(_) => Some(format!("{what} unwound into the caller")),
                Ok(Ok(())) => Some(format!("{what} accepted a panicking command")),
                Ok(Err(code)) if code.as_str() != "CMD-0009" => {
                    Some(format!("{what} refused it as {code}, not CMD-0009"))
                }
                Ok(Err(_)) => None,
            };
            if let Some(detail) = detail {
                out.push(Violation {
                    variant: variant.clone(),
                    rule: Rule::NotTotal,
                    detail,
                });
            }
        }
        if s.state_hash() != h0 {
            out.push(Violation {
                variant,
                rule: Rule::NotTotal,
                detail: "a panicking command changed the state".into(),
            });
        }
    }
    out
}
