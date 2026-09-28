//! The transport boundary between the editor's UI and its headless core (Ch.7.1, Ch.21
//! §21.18, Ch.34 §34.2).
//!
//! The UI never holds a [`forge_cmd::Bus`]. It holds a [`BusClient`]: it sends requests
//! (apply a command, open/commit/cancel a gesture's transaction, undo, redo) and it reads
//! back what happened — for **every** issuer — through [`BusClient::pump`], the `Applied`
//! stream plus the refusals of its own requests. [`crate::core::LocalBus`] is the
//! in-process client; the split editor's `RemoteBus` (M2-16) implements the same trait
//! over QUIC, which is why the split editor is a transport swap and not a rewrite.
//!
//! Requests return a [`Ticket`] and never a result: over a network the result arrives
//! later, so the UI is written to learn outcomes only from the stream. A local client
//! happens to deliver them in the same turn.

use std::sync::Arc;

use forge_cmd::{Applied, Diff, EditorCommand, Gap, Issuer, Project, Rejection, TxnId, TxnState};
use forge_project::status::ProjectStatus;
use serde::{Deserialize, Serialize};

/// Identifies one request of one client (a rejection names the ticket it refuses).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Ticket(pub u64);

/// One transaction as the undo history shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TxnInfo {
    pub txn: TxnId,
    pub label: String,
    pub issuer: Issuer,
    pub state: TxnState,
    /// When it was opened (ms since the Unix epoch, the bus clock).
    pub opened_at_ms: u64,
    pub undoable: bool,
}

/// A request of this client that the core refused. Nothing is dropped silently: the shell
/// turns every one into a notification carrying the stable `ErrorCode` (Ch.21 §21.18).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Refused {
    pub ticket: Ticket,
    /// What was asked, for the message ("Rename", "Undo \u{201c}Drag scale\u{201d}").
    pub what: String,
    pub rejection: Rejection,
}

/// A **session command** the core applied: an `Invoke` of a target the core registered as
/// a session target (the play controls, `forge.play.control`). It went through the bus —
/// validated, audited, issued by whoever sent it — but it changes session state that lives
/// beside the project (a play session), never the project, so its `Applied` event carries an
/// empty diff and the command itself reaches the clients that follow session commands here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCommand {
    /// The `seq` of the command's own `Applied` event: it takes effect after every event up
    /// to and including this one (a play started after an edit forks the edited world).
    pub seq: u64,
    pub issuer: Issuer,
    pub target: String,
    /// Its arguments (JSON), as the bus validated them.
    pub args: String,
}

/// What [`BusClient::pump`] returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pumped {
    /// Events were dropped since the last pump: resync from [`BusClient::snapshot`].
    pub gap: Option<Gap>,
    /// State changes, in `seq` order, from every issuer.
    pub events: Vec<Arc<Applied>>,
    /// This client's refused requests, in request order.
    pub refused: Vec<Refused>,
    /// Session commands from every issuer, in `seq` order, for a client that follows them
    /// ([`crate::core::LocalBus::follow_session`]).
    pub session: Vec<SessionCommand>,
    /// Session commands dropped because this client did not pump for too long (its inbox
    /// is bounded); the shell reports them — nothing is dropped silently.
    pub session_dropped: u64,
    /// The project lifecycle's status, when it changed since the last pump (an open,
    /// a save, a push, an outcome; WP-U7). A remote client carries it the same way.
    pub project: Option<Arc<ProjectStatus>>,
    /// **Optimistic predictions** (O-13): the diffs a remote client predicted for the
    /// requests it sent since the last pump, oldest first. The mirror shows each one as a
    /// pending overlay until its ticket is [`Pumped::settled`]. A local client predicts
    /// nothing: its answers arrive in the same turn.
    pub predicted: Vec<Prediction>,
    /// Requests whose authoritative outcome is in this pump or an earlier one (their
    /// events, or their refusal): the mirror drops their overlays.
    pub settled: Vec<Ticket>,
}

/// One optimistic prediction (see [`Pumped::predicted`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Prediction {
    pub ticket: Ticket,
    /// The dry run of the request's command against the client's replica.
    pub diff: Diff,
}

impl Pumped {
    pub fn is_empty(&self) -> bool {
        self.gap.is_none()
            && self.events.is_empty()
            && self.refused.is_empty()
            && self.session.is_empty()
            && self.session_dropped == 0
            && self.project.is_none()
            && self.predicted.is_empty()
            && self.settled.is_empty()
    }
}

/// A client of the editor core (see the module docs). The issuer is fixed when the client
/// is connected: a panel cannot forge one.
pub trait BusClient {
    /// Who this client's requests are issued as.
    fn issuer(&self) -> &Issuer;
    /// Send one command. `txn`: inside an open gesture transaction; `None`: its own
    /// transaction (one undo entry).
    fn apply(&mut self, cmd: EditorCommand, txn: Option<TxnId>) -> Ticket;
    /// Open a transaction (a gesture, or several commands that are one user intent).
    fn begin(&mut self, label: &str) -> TxnId;
    fn commit(&mut self, txn: TxnId) -> Ticket;
    /// Cancel an open transaction (Esc during a gesture): its changes are reverted.
    fn cancel(&mut self, txn: TxnId) -> Ticket;
    fn undo(&mut self, txn: TxnId) -> Ticket;
    fn redo(&mut self, txn: TxnId) -> Ticket;
    /// What Ctrl+Z would undo now.
    fn undo_target(&self) -> Option<TxnId>;
    /// What Ctrl+Y would redo now.
    fn redo_target(&self) -> Option<TxnId>;
    /// The effect of `cmd` without applying it (validation, hover previews, the split
    /// editor's optimistic overlay).
    fn preview(&self, cmd: &EditorCommand) -> Result<Diff, Rejection>;
    /// One transaction's history record.
    fn txn_info(&self, txn: TxnId) -> Option<TxnInfo>;
    /// The undo history, oldest first (a mirror's resync).
    fn history(&self) -> Vec<TxnInfo>;
    /// The whole project and the `seq` of the next event (a mirror's first sync or its
    /// resync after a gap): events below that `seq` are already in the snapshot.
    fn snapshot(&self) -> (Project, u64);
    /// Everything that happened since the last pump.
    fn pump(&mut self) -> Pumped;
    /// Receive session commands ([`SessionCommand`]) from every issuer from now on (the
    /// shell does: it hosts the play core).
    fn follow_session(&mut self, on: bool);
    /// The project lifecycle's status now (a resync; [`Pumped::project`] carries changes).
    /// `None`: this client's core has no project host.
    fn project_status(&self) -> Option<Arc<ProjectStatus>> {
        None
    }
}
