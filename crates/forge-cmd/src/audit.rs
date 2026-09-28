//! Provenance: the audit log and its clock.

use std::time::{SystemTime, UNIX_EPOCH};

use forge_core::ErrorCode;

use crate::{CommandId, EditorCommand, Issuer, TxnId};

/// Wall-clock source for audit timestamps. Injected so tests are reproducible; the time is
/// provenance, never project content, so it does not touch determinism (Ch.3).
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
}

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

/// A clock that always reads the same time (tests, replays).
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

/// What an audit entry records.
#[derive(Clone, Debug, PartialEq)]
pub enum AuditAction {
    /// A command was sent (applied or refused — see the outcome).
    Command(EditorCommand),
    /// A transaction was opened, with its label.
    Begin(String),
    /// A transaction was committed.
    Commit,
    /// An open transaction was cancelled.
    Cancel,
    /// A transaction was undone.
    Undo,
    /// A transaction was redone.
    Redo,
}

/// One line of the audit trail. Every bus operation is recorded — **including every
/// refusal** — so nothing an automation session, a script or a teammate tried is invisible.
///
/// Inside an open gesture, the per-frame commands on one target (the same property, setting,
/// entity name or `Invoke` handler) with the same outcome fold into **one** entry: `merged`
/// counts the frames, `first_command`..`command` spans their ids, `at_ms`..`last_at_ms`
/// their times, and `action` / `outcome` are the latest frame's. A 10 000-frame drag is one
/// line per property, not 10 000 command clones. Folding happens only while the gesture's
/// own lines are the newest in the trail: any other entry (another transaction, someone
/// else's refused attempt) closes them, and the next frame starts a new line after it.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditEntry {
    /// Order on this bus (shared with `Applied::seq` numbering only in being monotonic).
    pub seq: u64,
    /// When, per the bus's [`Clock`] (the first frame, for a folded entry).
    pub at_ms: u64,
    /// When the latest frame folded into it happened (equal to `at_ms` for a single
    /// operation). A folded entry spans `at_ms..=last_at_ms`, and no other entry in the
    /// trail falls strictly inside that span: folding never reorders the trail.
    pub last_at_ms: u64,
    /// Who asked.
    pub issuer: Issuer,
    /// The transaction.
    pub txn: TxnId,
    /// The command, if the action was one (the latest frame, for a folded entry).
    pub command: Option<CommandId>,
    /// The first frame's command, for a folded entry; equal to `command` otherwise.
    pub first_command: Option<CommandId>,
    /// How many operations this entry stands for (1 unless gesture frames folded into it).
    pub merged: u64,
    /// What was asked.
    pub action: AuditAction,
    /// `Ok(number of changes applied)` or `Err(code)`.
    pub outcome: Result<usize, ErrorCode>,
}
