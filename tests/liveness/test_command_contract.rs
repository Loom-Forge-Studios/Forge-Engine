//! I8 — `test_command_contract` (Ch.7.3): every `EditorCommand` variant has a `dry_run` that
//! does not mutate (state hash before/after), agrees with `apply` (the optimistic preview is
//! exact, O-13), produces an undo entry, is undone and redone exactly, is provenance-tagged,
//! and round-trips through serde (the wire) and reflection.
//!
//! The variant list comes from `EditorCommand`'s reflected `TypeInfo`, not from a list kept
//! next to the samples, so a new variant without a sample fails the guard.
//!
//! Totality: `dry_run` and `apply` of a panicking command (the fixture's `fixture.panic`
//! handler) each return a `CMD-0009` rejection and leave the state untouched; a bus whose
//! planning lost its `catch_unwind` unwinds instead, and this guard fails.
//!
//! Positive control (W2): `positive_control_every_clause_catches_its_fault` runs the same
//! check on sinks that each break exactly one clause, and on sample lists that are missing a
//! variant or are vacuous; every one must be caught, by the clause that names it.

use std::cell::RefCell;
use std::sync::Arc;

use forge_cmd::contract::{
    self, ContractSubject, Rule, Violation, check_contract, check_coverage, check_totality,
    fixture, panicking_samples, samples,
};
use forge_cmd::{
    Applied, Bus, CommandEnvelope, CommandId, CommandSink, Diff, EditorCommand, Issuer, Rejection,
    TxnId, Value,
};

#[test]
fn every_editor_command_honours_the_contract() {
    let s = samples();
    let coverage = check_coverage(&s);
    assert!(
        coverage.is_empty(),
        "variants without a sample: {coverage:#?}"
    );
    let violations = check_contract(fixture, &s);
    assert!(
        violations.is_empty(),
        "I8 violated:\n{}",
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Totality: a panicking command is a CMD-0009 rejection from both dry_run and apply,
    // never an unwind (the bus's catch_unwind is load-bearing: remove it and this fails).
    let total = check_totality(fixture, &panicking_samples());
    assert!(total.is_empty(), "I8 totality violated: {total:#?}");
    // Non-vacuity: the reflected variant list is the real one, and every sample was checked.
    let declared = contract::reflected_variants();
    assert!(declared.len() >= 8, "{declared:?}");
    assert!(declared.contains(&"SetProperty") && declared.contains(&"Invoke"));
    assert_eq!(s.len(), declared.len());
}

// ---- positive controls ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fault {
    /// `dry_run` applies the command.
    DryRunApplies,
    /// `dry_run` returns an empty diff (a preview that lies).
    DryRunEmpty,
    /// `apply` forgets its undo entry.
    ForgetsHistory,
    /// `undo` reports success and does nothing.
    UndoNoop,
    /// `redo` reports success and does nothing.
    RedoNoop,
    /// `apply` reports an anonymous issuer.
    Anonymous,
    /// The wire drops the command id.
    LossyWire,
    /// `dry_run` and `apply` plan without catching a panic (the bus without catch_unwind).
    Unwinds,
}

/// A real bus with one fault injected.
struct Broken {
    inner: RefCell<Bus>,
    fault: Fault,
}

impl Broken {
    fn new(fault: Fault) -> Self {
        Self {
            inner: RefCell::new(fixture()),
            fault,
        }
    }
}

impl CommandSink for Broken {
    fn dry_run(&self, e: &CommandEnvelope) -> Result<Diff, Rejection> {
        unwind_if_panicking(self.fault, e);
        match self.fault {
            Fault::DryRunApplies => self
                .inner
                .borrow_mut()
                .apply(e.clone())
                .map(|a| a.diff.clone()),
            Fault::DryRunEmpty => self.inner.borrow().dry_run(e).map(|_| Diff::default()),
            _ => self.inner.borrow().dry_run(e),
        }
    }

    fn apply(&mut self, e: CommandEnvelope) -> Result<Arc<Applied>, Rejection> {
        unwind_if_panicking(self.fault, &e);
        let mut bus = self.inner.borrow_mut();
        let a = bus.apply(e)?;
        match self.fault {
            Fault::ForgetsHistory => {
                bus.clear_history();
                Ok(a)
            }
            Fault::Anonymous => {
                let mut a = (*a).clone();
                a.issuer = Issuer::Human { user: "?".into() };
                Ok(Arc::new(a))
            }
            _ => Ok(a),
        }
    }

    fn undo(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection> {
        if self.fault == Fault::UndoNoop {
            return Ok(fake(txn));
        }
        self.inner.borrow_mut().undo(txn)
    }

    fn redo(&mut self, txn: TxnId) -> Result<Arc<Applied>, Rejection> {
        if self.fault == Fault::RedoNoop {
            return Ok(fake(txn));
        }
        self.inner.borrow_mut().redo(txn)
    }
}

/// What a bus without `catch_unwind` around planning does with a panicking handler.
fn unwind_if_panicking(fault: Fault, e: &CommandEnvelope) {
    if fault == Fault::Unwinds
        && matches!(&e.cmd, EditorCommand::Invoke { target, .. } if target == contract::FIXTURE_PANIC)
    {
        panic!("fixture.panic: unwound through a sink without catch_unwind");
    }
}

fn fake(txn: TxnId) -> Arc<Applied> {
    Arc::new(Applied {
        seq: 0,
        kind: forge_cmd::AppliedKind::Undo,
        txn,
        command: None,
        issuer: Issuer::Test,
        diff: Diff::default(),
    })
}

impl ContractSubject for Broken {
    fn state_hash(&self) -> u64 {
        self.inner.borrow().state_hash()
    }

    fn undo_depth(&self) -> usize {
        self.inner.borrow().undo_depth()
    }

    fn envelope(&mut self, cmd: EditorCommand) -> CommandEnvelope {
        self.inner.borrow_mut().envelope(Issuer::Test, cmd)
    }

    fn wire_roundtrip(&self, e: &CommandEnvelope) -> Result<CommandEnvelope, String> {
        let mut back = self.inner.borrow().wire_roundtrip(e)?;
        if self.fault == Fault::LossyWire {
            back.id = CommandId(0);
        }
        Ok(back)
    }
}

fn rules(v: &[Violation]) -> Vec<Rule> {
    v.iter().map(|x| x.rule).collect()
}

#[test]
fn positive_control_every_clause_catches_its_fault() {
    let s = samples();
    for (fault, rule) in [
        (Fault::DryRunApplies, Rule::DryRunMutated),
        (Fault::DryRunEmpty, Rule::DryRunDisagreesWithApply),
        (Fault::ForgetsHistory, Rule::NoUndoEntry),
        (Fault::UndoNoop, Rule::UndoDidNotRestore),
        (Fault::RedoNoop, Rule::RedoDidNotRestore),
        (Fault::Anonymous, Rule::NotProvenanceTagged),
        (Fault::LossyWire, Rule::SerdeRoundTrip),
    ] {
        let v = check_contract(|| Broken::new(fault), &s);
        let caught = rules(&v);
        assert!(
            caught.contains(&rule),
            "{fault:?} was not caught as {rule:?}; got {caught:?}"
        );
        // Every variant is affected by every fault, so each must be reported per variant.
        let per_variant = v.iter().filter(|x| x.rule == rule).count();
        assert_eq!(per_variant, s.len(), "{fault:?}: {v:#?}");
    }

    // An unbroken wrapper reports nothing: the faults, not the wrapper, trip the guard.
    struct Clean(Broken);
    impl CommandSink for Clean {
        fn dry_run(&self, e: &CommandEnvelope) -> Result<Diff, Rejection> {
            self.0.inner.borrow().dry_run(e)
        }
        fn apply(&mut self, e: CommandEnvelope) -> Result<Arc<Applied>, Rejection> {
            self.0.inner.borrow_mut().apply(e)
        }
        fn undo(&mut self, t: TxnId) -> Result<Arc<Applied>, Rejection> {
            self.0.inner.borrow_mut().undo(t)
        }
        fn redo(&mut self, t: TxnId) -> Result<Arc<Applied>, Rejection> {
            self.0.inner.borrow_mut().redo(t)
        }
    }
    impl ContractSubject for Clean {
        fn state_hash(&self) -> u64 {
            self.0.state_hash()
        }
        fn undo_depth(&self) -> usize {
            self.0.undo_depth()
        }
        fn envelope(&mut self, cmd: EditorCommand) -> CommandEnvelope {
            self.0.envelope(cmd)
        }
    }
    let v = check_contract(|| Clean(Broken::new(Fault::DryRunEmpty)), &s);
    assert!(v.is_empty(), "{v:#?}");

    // The totality clause catches a sink that lets a panicking command unwind, on both
    // dry_run and apply; the clean wrapper passes it.
    let v = check_totality(|| Broken::new(Fault::Unwinds), &panicking_samples());
    let details: Vec<&str> = v.iter().map(|x| x.detail.as_str()).collect();
    assert_eq!(rules(&v), vec![Rule::NotTotal, Rule::NotTotal], "{v:#?}");
    assert!(details[0].starts_with("dry_run unwound"), "{details:?}");
    assert!(details[1].starts_with("apply unwound"), "{details:?}");
    let v = check_totality(|| Clean(Broken::new(Fault::Unwinds)), &panicking_samples());
    assert!(v.is_empty(), "{v:#?}");
    // A command that does not panic is not a totality sample: accepting it is flagged, so
    // the clause cannot pass vacuously on a harmless sample.
    let v = check_totality(fixture, &s[..1]);
    assert!(rules(&v).contains(&Rule::NotTotal), "{v:#?}");

    // A variant without a sample is caught (the list comes from reflection).
    let missing: Vec<EditorCommand> = s
        .iter()
        .filter(|c| !matches!(c, EditorCommand::Invoke { .. }))
        .cloned()
        .collect();
    let v = check_coverage(&missing);
    assert_eq!(v.len(), 1, "{v:#?}");
    assert_eq!(
        (v[0].rule, v[0].variant.as_str()),
        (Rule::MissingSample, "Invoke")
    );

    // A vacuous sample (changes nothing) and an invalid one are caught, not counted as passes.
    let noop = EditorCommand::SetProperty {
        entity: contract::PLAYER,
        path: "hp".into(),
        value: Value::Int(10), // the fixture's value already
    };
    assert_eq!(
        rules(&check_contract(fixture, &[noop])),
        vec![Rule::SampleIsNoop]
    );
    let invalid = EditorCommand::Despawn {
        entity: forge_cmd::EntityKey(999),
    };
    assert_eq!(
        rules(&check_contract(fixture, &[invalid])),
        vec![Rule::SampleRejected]
    );
}
