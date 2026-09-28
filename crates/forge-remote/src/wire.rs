//! The wire (Ch.34 §34.3): what crosses between a remote client and the host, and how it is
//! framed.
//!
//! One bidirectional QUIC stream per connection carries everything, in order, so an
//! answer never overtakes the events it follows. A frame is a little-endian `u32` length and
//! a `postcard` body; nothing longer than [`MAX_FRAME`] is read. The payload types are the
//! bus's own (`EditorCommand`, `Applied`, `Rejection`, `Project`'s canonical wire form), so a
//! delta is exactly the reflect-tree diff the core applied — small, and already there.
//!
//! A remote transaction the client opened is named by an **alias** until the host answers
//! with its real id ([`HostMsg::Begun`]): a gesture starts without waiting a round trip.

use std::sync::Arc;

use forge_cmd::{Applied, Diff, EditorCommand, Gap, Issuer, Project, Rejection, TxnId, Value};
use forge_editor::client::{Refused, SessionCommand, Ticket, TxnInfo};
use forge_project::status::ProjectStatus;
use serde::{Deserialize, Serialize};

use crate::RemoteError;

/// The largest frame either side reads (a snapshot of a large project fits; a hostile length
/// prefix does not make the reader allocate gigabytes).
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// The bit that marks a client-side alias in a [`TxnId`] (see the module docs). The bus
/// allocates ids from 0 upward and never reaches it.
pub const ALIAS_BIT: u64 = 1 << 62;

/// Whether `t` is a client alias.
#[must_use]
pub const fn is_alias(t: TxnId) -> bool {
    t.0 & ALIAS_BIT != 0
}

/// The first message a device sends on a connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Hello {
    /// Ask to pair: the device's name (what the person here sees) and its SPAKE2 message
    /// over the code its person typed.
    Pair { name: String, spake: Vec<u8> },
    /// Open an editor session (a paired device only). `follow_session`: deliver session
    /// commands (the play controls) to this client, as a shell that hosts a play core wants.
    Session { follow_session: bool },
}

/// The host's side of a pairing exchange.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PairMsg {
    /// The host's SPAKE2 message and its key confirmation.
    Spake { spake: Vec<u8>, confirm: Vec<u8> },
    /// Pairing cannot start (no code on show, a malformed message).
    Refused { why: String },
    /// The device's key confirmation arrived; a person here now allows or denies it.
    Waiting,
    /// A person allowed it: this is the device's id on the host.
    Paired { device_id: String },
    /// A person denied it, or the code was wrong or expired.
    Denied { why: String },
}

/// The device's key confirmation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfirm {
    pub confirm: Vec<u8>,
}

/// A request of a remote session (client to host).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Apply {
        ticket: Ticket,
        cmd: EditorCommand,
        txn: Option<TxnId>,
    },
    /// Open a transaction; `alias` names it until [`HostMsg::Begun`].
    Begin {
        alias: u64,
        label: String,
    },
    Commit {
        ticket: Ticket,
        txn: TxnId,
    },
    Cancel {
        ticket: Ticket,
        txn: TxnId,
    },
    Undo {
        ticket: Ticket,
        txn: TxnId,
    },
    Redo {
        ticket: Ticket,
        txn: TxnId,
    },
    /// A dry run the client could not plan itself (a plugin command only the core has).
    Preview {
        req: u64,
        cmd: EditorCommand,
    },
    FollowSession(bool),
    /// The client's replica missed something: send a snapshot.
    Resync,
}

/// What the host sends a session (host to client).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HostMsg {
    /// The session is open: who the client is and the state to start from.
    Welcome(Box<Welcome>),
    /// What happened since the last batch, in order.
    Batch(Box<Batch>),
    /// The client fell behind (or asked): start again from this snapshot.
    Snapshot(Box<Snapshot>),
    /// A transaction the client opened as `alias` is `txn`.
    Begun { alias: u64, txn: TxnId },
    /// The answer to [`Request::Preview`].
    Preview {
        req: u64,
        result: Result<Diff, Rejection>,
    },
    /// The host ends the session.
    Closed { why: String },
}

/// See [`HostMsg::Welcome`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Welcome {
    pub session: String,
    pub device_id: String,
    /// Who this client's requests are issued as (fixed by the host from the pairing: a
    /// client cannot choose it).
    pub issuer: Issuer,
    pub project: Project,
    pub next_seq: u64,
    pub history: Vec<TxnInfo>,
    pub undo: Option<TxnId>,
    pub redo: Option<TxnId>,
    pub status: Option<ProjectStatus>,
}

/// See [`HostMsg::Batch`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub events: Vec<Applied>,
    /// The records of the transactions the events name (the history panel's rows).
    pub infos: Vec<TxnInfo>,
    /// Requests whose outcome is in this batch or was in an earlier one.
    pub settled: Vec<Ticket>,
    pub refused: Vec<Refused>,
    pub session: Vec<SessionCommand>,
    pub session_dropped: u64,
    pub undo: Option<TxnId>,
    pub redo: Option<TxnId>,
    pub status: Option<ProjectStatus>,
    /// The whole undo history, when a project load replaced it.
    pub history: Option<Vec<TxnInfo>>,
}

/// See [`HostMsg::Snapshot`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub gap: Option<Gap>,
    pub project: Project,
    pub next_seq: u64,
    pub history: Vec<TxnInfo>,
    pub undo: Option<TxnId>,
    pub redo: Option<TxnId>,
}

forge_trace::control_switches! {
    /// W2 positive-control switches for the wire (`test_split_editor_parity`'s control). Never
    /// set outside those tests.
    #[doc(hidden)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct WireFaults {
        /// Floats in commands cross the wire as `f32`: the lossy encoding a hand-rolled
        /// "compact" wire might pick. Parity must catch it.
        pub f32_floats: bool,
    }
}

impl WireFaults {
    /// `cmd` as this (faulty) wire would carry it.
    #[must_use]
    pub fn mangle(self, cmd: EditorCommand) -> EditorCommand {
        if !self.f32_floats() {
            return cmd;
        }
        let lossy = |v: Value| match v {
            Value::Float(x) => Value::Float(f64::from(x as f32)),
            Value::Vec3(a) => Value::Vec3(a.map(|x| f64::from(x as f32))),
            other => other,
        };
        match cmd {
            EditorCommand::SetProperty {
                entity,
                path,
                value,
            } => EditorCommand::SetProperty {
                entity,
                path,
                value: lossy(value),
            },
            EditorCommand::SetSetting { key, value } => EditorCommand::SetSetting {
                key,
                value: value.map(lossy),
            },
            other => other,
        }
    }
}

/// Encode one frame body.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, RemoteError> {
    let body = postcard::to_stdvec(msg).map_err(|e| RemoteError::Protocol(e.to_string()))?;
    if body.len() > MAX_FRAME {
        return Err(RemoteError::Protocol(format!(
            "a {} byte frame is over the {MAX_FRAME} byte limit",
            body.len()
        )));
    }
    let mut out = Vec::with_capacity(body.len() + 4);
    let len = u32::try_from(body.len()).map_err(|e| RemoteError::Protocol(e.to_string()))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// The SHA-256 of `p`'s canonical wire form, lowercase hex: two projects with the same
/// digest are byte-identical (the headless host's console prints it; the split-editor parity
/// tests compare it across processes).
pub fn project_digest(p: &Project) -> Result<String, RemoteError> {
    use sha2::Digest as _;
    let body = postcard::to_stdvec(p).map_err(|e| RemoteError::Protocol(e.to_string()))?;
    Ok(crate::hex(&sha2::Sha256::digest(&body)))
}

/// Decode one frame body.
pub fn decode<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, RemoteError> {
    postcard::from_bytes(body).map_err(|e| RemoteError::Protocol(e.to_string()))
}

/// Write one message as a frame.
pub async fn send<T: Serialize>(s: &mut quinn::SendStream, msg: &T) -> Result<(), RemoteError> {
    let frame = encode(msg)?;
    s.write_all(&frame)
        .await
        .map_err(|e| RemoteError::Transport(e.to_string()))
}

/// Read one frame and decode it. `Ok(None)`: the peer finished the stream cleanly.
pub async fn recv<T: for<'de> Deserialize<'de>>(
    r: &mut quinn::RecvStream,
) -> Result<Option<T>, RemoteError> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(RemoteError::Transport(e.to_string())),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(RemoteError::Protocol(format!(
            "the peer announced a {n} byte frame (limit {MAX_FRAME})"
        )));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)
        .await
        .map_err(|e| RemoteError::Transport(e.to_string()))?;
    decode(&body).map(Some)
}

/// The events of a batch as the client hands them on (shared, like the bus's).
#[must_use]
pub fn shared(events: Vec<Applied>) -> Vec<Arc<Applied>> {
    events.into_iter().map(Arc::new).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cmd::{AppliedKind, Change, CommandId, EntityKey};

    #[test]
    fn a_batch_round_trips_bit_exact() {
        let b = Batch {
            events: vec![Applied {
                seq: 4,
                kind: AppliedKind::Command,
                txn: TxnId(2),
                command: Some(CommandId(9)),
                issuer: Issuer::Human { user: "ada".into() },
                diff: Diff {
                    changes: vec![Change::Property {
                        entity: EntityKey(1),
                        path: "transform.position".into(),
                        before: None,
                        after: Some(Value::Vec3([0.1, -0.0, 1e-300])),
                    }],
                },
            }],
            settled: vec![Ticket(3)],
            ..Batch::default()
        };
        let frame = encode(&HostMsg::Batch(Box::new(b.clone()))).expect("encodes");
        let back: HostMsg = decode(&frame[4..]).expect("decodes");
        match back {
            HostMsg::Batch(got) => assert_eq!(*got, b),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_f32_fault_really_loses_bits() {
        let cmd = EditorCommand::SetSetting {
            key: "a".into(),
            value: Some(Value::Float(0.1)),
        };
        let lossy = WireFaults { f32_floats: true }.mangle(cmd.clone());
        assert_ne!(lossy, cmd);
        assert_eq!(WireFaults::default().mangle(cmd.clone()), cmd);
    }
}
