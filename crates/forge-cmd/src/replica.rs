//! [`Replica`] — a client's copy of a bus's project, kept current by the `Applied` stream,
//! carrying **predictions** on top (the split editor's optimistic preview, O-13, Ch.34).
//!
//! A remote client cannot wait a round trip to see its own slider drag. It plans each
//! command locally against the replica ([`crate::Bus::plan_for`]: the same planners the
//! core runs, so the prediction *is* the dry run), applies the planned diff on top as a
//! pending prediction tagged with the request, and sends the command. When the core's
//! answer arrives, [`Replica::reconcile`] rolls every pending prediction back (newest first,
//! by their inverses), applies the authoritative events in `seq` order, forgets the
//! predictions whose requests are settled, and re-applies the rest. A prediction that was
//! wrong is therefore corrected by the very next reconcile — the one-frame correction of
//! network prediction — and one that was right costs nothing but the bookkeeping.
//!
//! The replica never feeds anything back: it is not a sink, and the bus that owns the real
//! project is not reachable from it (I7).

use std::sync::Arc;

use crate::{Applied, CmdError, Diff, Project};

/// See the module docs.
#[derive(Clone, Debug, Default)]
pub struct Replica {
    /// The authoritative project as of `next_seq`, with `pending` applied on top.
    project: Project,
    next_seq: u64,
    /// Predictions not yet settled, oldest first: `(tag, diff)`.
    pending: Vec<(u64, Diff)>,
    /// The key allocator before the first pending prediction (restored on rollback: a
    /// predicted spawn must not move the allocator the core owns).
    base_next_key: u64,
}

impl Replica {
    /// A replica of `project` whose next expected event is `next_seq` (a snapshot).
    #[must_use]
    pub fn new(project: Project, next_seq: u64) -> Self {
        let base_next_key = project.raw_next_key();
        Self {
            project,
            next_seq,
            pending: Vec::new(),
            base_next_key,
        }
    }

    /// The project as the client should show it: authoritative state plus every pending
    /// prediction.
    #[must_use]
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The `seq` of the next authoritative event expected.
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Predictions not yet settled.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// The tags of the pending predictions, oldest first.
    pub fn pending_tags(&self) -> impl Iterator<Item = u64> + '_ {
        self.pending.iter().map(|(t, _)| *t)
    }

    /// Apply `diff` on top as the prediction for request `tag`. A diff that does not apply to
    /// the predicted state is refused and nothing changes (the request is still sent; the
    /// core's answer is what the client then shows).
    pub fn predict(&mut self, tag: u64, diff: Diff) -> Result<(), CmdError> {
        if self.pending.is_empty() {
            self.base_next_key = self.project.raw_next_key();
        }
        self.project.apply_all(&diff.changes)?;
        self.pending.push((tag, diff));
        Ok(())
    }

    /// Roll every pending prediction back (newest first). `Err` only if the replica was
    /// changed underneath its predictions, which nothing outside this type can do.
    fn rollback(project: &mut Project, pending: &[(u64, Diff)], base: u64) -> Result<(), CmdError> {
        for (_, d) in pending.iter().rev() {
            project.apply_all(&d.inverse().changes)?;
        }
        if !pending.is_empty() {
            project.set_raw_next_key(base);
        }
        Ok(())
    }

    /// Re-apply the pending predictions on the authoritative state; one that no longer
    /// applies (the core changed what it predicted from) is dropped — its request's answer
    /// will show what really happened.
    fn reapply(&mut self) {
        self.base_next_key = self.project.raw_next_key();
        let project = &mut self.project;
        self.pending
            .retain(|(_, d)| project.apply_all(&d.changes).is_ok());
    }

    /// Follow the core: roll the predictions back, apply `events` (those below
    /// [`Replica::next_seq`] are already in), drop the predictions of the `settled` requests,
    /// re-apply the rest. `Err`: an authoritative event did not apply (the replica missed
    /// something): the client must resync from a snapshot ([`Replica::resync`]).
    pub fn reconcile(&mut self, events: &[Arc<Applied>], settled: &[u64]) -> Result<(), CmdError> {
        if events.is_empty() && settled.is_empty() {
            return Ok(());
        }
        Self::rollback(&mut self.project, &self.pending, self.base_next_key)?;
        for e in events {
            if e.seq < self.next_seq {
                continue;
            }
            self.project.apply_all(&e.diff.changes)?;
            self.next_seq = e.seq + 1;
        }
        self.pending.retain(|(t, _)| !settled.contains(t));
        self.reapply();
        Ok(())
    }

    /// The authoritative project (the predictions rolled back, on a copy).
    #[must_use]
    pub fn authoritative(&self) -> Project {
        let mut p = self.project.clone();
        if Self::rollback(&mut p, &self.pending, self.base_next_key).is_err() {
            // Unreachable (see `rollback`); the predicted state is the closest there is.
            return self.project.clone();
        }
        p
    }

    /// Replace the authoritative state with a snapshot (`project` as of `next_seq`) and
    /// re-apply the pending predictions on it.
    pub fn resync(&mut self, project: Project, next_seq: u64) {
        self.project = project;
        self.next_seq = next_seq;
        self.reapply();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppliedKind, Change, EntityKey, Issuer, TxnId, Value};

    fn ev(seq: u64, changes: Vec<Change>) -> Arc<Applied> {
        Arc::new(Applied {
            seq,
            kind: AppliedKind::Command,
            txn: TxnId(seq),
            command: None,
            issuer: Issuer::Test,
            diff: Diff { changes },
        })
    }

    fn set(key: &str, before: Option<i64>, after: Option<i64>) -> Change {
        Change::Setting {
            key: key.into(),
            before: before.map(Value::Int),
            after: after.map(Value::Int),
        }
    }

    #[test]
    fn a_wrong_prediction_is_corrected_by_the_next_reconcile() {
        let mut r = Replica::new(Project::new(), 0);
        r.predict(
            1,
            Diff {
                changes: vec![set("a", None, Some(50))],
            },
        )
        .expect("applies");
        assert_eq!(r.project().setting("a"), Some(&Value::Int(50)));
        assert_eq!(r.authoritative().setting("a"), None);
        // The core clamped it.
        r.reconcile(&[ev(0, vec![set("a", None, Some(10))])], &[1])
            .expect("reconciles");
        assert_eq!(r.project().setting("a"), Some(&Value::Int(10)));
        assert_eq!(r.pending(), 0);
    }

    #[test]
    fn a_foreign_event_lands_under_a_pending_prediction() {
        let mut r = Replica::new(Project::new(), 0);
        r.predict(
            7,
            Diff {
                changes: vec![set("mine", None, Some(1))],
            },
        )
        .expect("applies");
        r.reconcile(&[ev(0, vec![set("theirs", None, Some(2))])], &[])
            .expect("reconciles");
        assert_eq!(
            r.project().setting("mine"),
            Some(&Value::Int(1)),
            "still predicted"
        );
        assert_eq!(r.project().setting("theirs"), Some(&Value::Int(2)));
        assert_eq!(r.authoritative().setting("mine"), None);
        assert_eq!(r.next_seq(), 1);
    }

    #[test]
    fn a_predicted_spawn_does_not_move_the_allocator() {
        let mut r = Replica::new(Project::new(), 0);
        let spawn = Change::Created {
            entity: EntityKey(0),
            name: "Cube".into(),
            parent: None,
        };
        r.predict(
            1,
            Diff {
                changes: vec![spawn],
            },
        )
        .expect("applies");
        assert_eq!(r.project().next_key(), EntityKey(1));
        // Refused by the core: nothing happened.
        r.reconcile(&[], &[1]).expect("reconciles");
        assert!(r.project().is_empty());
        assert_eq!(r.project().next_key(), EntityKey(0));
    }
}
