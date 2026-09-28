//! `CmdError` (the `CMD-*` codes, `docs/error-codes.md`) and [`Rejection`], the bus's answer
//! to a command it will not apply.

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};
use serde::{Deserialize, Serialize};

use crate::{CommandId, EntityKey, TxnId};

/// The state a transaction is in, for [`CmdError::TxnState`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TxnState {
    /// Begun and accepting commands.
    Open,
    /// Committed; undoable.
    Committed,
    /// Undone; redoable.
    Undone,
    /// Cancelled while open (Esc during a gesture); gone for good.
    Cancelled,
    /// Committed, then no longer undoable: it dropped off the end of the bounded history,
    /// the history was cleared, or it changed nothing (so it never entered the history).
    Expired,
}

impl fmt::Display for TxnState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "open",
            Self::Committed => "committed",
            Self::Undone => "undone",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        })
    }
}

/// Every way the bus can refuse. Converts into `forge_core::Error` with `?`. Serializable:
/// the split editor's wire carries a refusal to the client that sent the command (M2-16).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CmdError {
    /// `CMD-0001`: no project entity has this key (never spawned, or despawned).
    UnknownEntity(EntityKey),
    /// `CMD-0002`: the reparent would make an entity its own ancestor.
    Cycle {
        /// The entity being moved.
        entity: EntityKey,
        /// The parent it was to move under.
        parent: EntityKey,
    },
    /// `CMD-0003`: a property path or setting key is not `ident(.ident)*`.
    BadPath(String),
    /// `CMD-0004`: a value contains NaN or an infinity.
    NonFinite {
        /// Where it was going.
        path: String,
    },
    /// `CMD-0005`: no transaction has this id on this bus.
    UnknownTxn(TxnId),
    /// `CMD-0006`: the operation needs the transaction in another state (a command sent into
    /// a committed transaction, an undo of an undone one, a redo of a committed one).
    TxnState {
        /// The transaction.
        txn: TxnId,
        /// The state the operation needs.
        expected: TxnState,
        /// The state it is in.
        found: TxnState,
    },
    /// `CMD-0007`: an entity name is empty, too long, or contains a control character.
    BadName(String),
    /// `CMD-0008`: the project changed underneath: a diff's `before` no longer matches the
    /// project (an undo of a transaction whose values were since overwritten, or a handler
    /// that described a change inconsistently). Nothing was applied.
    Conflict {
        /// What did not match.
        detail: String,
    },
    /// `CMD-0009`: the command panicked. The bus caught it (Ch.1.2); nothing was applied.
    Panicked {
        /// The panic message.
        message: String,
    },
    /// `CMD-0010`: `Invoke` names a target no handler is registered for.
    UnknownHandler(String),
    /// `CMD-0011`: `Invoke` arguments are not valid for the target.
    BadArgs {
        /// The target.
        target: String,
        /// Why.
        why: String,
    },
    /// `CMD-0012`: a command's issuer differs from its transaction's issuer (an automation session
    /// cannot append to a human's gesture).
    IssuerMismatch {
        /// The transaction.
        txn: TxnId,
    },
    /// `CMD-0013`: an envelope with this command id was already applied, or its id is older
    /// than the bus's replay window (`REPLAY_WINDOW`) — a stale resend.
    DuplicateCommand(CommandId),
    /// `CMD-0014`: refused by the command's policy: a security-class command from a
    /// non-human issuer, or an undo/redo the command forbids (Ch.21.18: undo never grants).
    PolicyRefused {
        /// What was refused, and why.
        what: String,
    },
    /// `CMD-0015`: a previous failure left the project in a state the bus could not prove
    /// consistent; the bus refuses every command rather than build on it.
    Poisoned,
    /// `CMD-0016`: a handler with this target is already registered.
    DuplicateHandler(String),
}

impl CmdError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::UnknownEntity(_) => error_code!("CMD-0001"),
            Self::Cycle { .. } => error_code!("CMD-0002"),
            Self::BadPath(_) => error_code!("CMD-0003"),
            Self::NonFinite { .. } => error_code!("CMD-0004"),
            Self::UnknownTxn(_) => error_code!("CMD-0005"),
            Self::TxnState { .. } => error_code!("CMD-0006"),
            Self::BadName(_) => error_code!("CMD-0007"),
            Self::Conflict { .. } => error_code!("CMD-0008"),
            Self::Panicked { .. } => error_code!("CMD-0009"),
            Self::UnknownHandler(_) => error_code!("CMD-0010"),
            Self::BadArgs { .. } => error_code!("CMD-0011"),
            Self::IssuerMismatch { .. } => error_code!("CMD-0012"),
            Self::DuplicateCommand(_) => error_code!("CMD-0013"),
            Self::PolicyRefused { .. } => error_code!("CMD-0014"),
            Self::Poisoned => error_code!("CMD-0015"),
            Self::DuplicateHandler(_) => error_code!("CMD-0016"),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<CmdError> {
        let t = TxnId(1);
        vec![
            Self::UnknownEntity(EntityKey(1)),
            Self::Cycle {
                entity: EntityKey(1),
                parent: EntityKey(2),
            },
            Self::BadPath("a..b".into()),
            Self::NonFinite { path: "x".into() },
            Self::UnknownTxn(t),
            Self::TxnState {
                txn: t,
                expected: TxnState::Open,
                found: TxnState::Committed,
            },
            Self::BadName(String::new()),
            Self::Conflict { detail: "x".into() },
            Self::Panicked {
                message: "boom".into(),
            },
            Self::UnknownHandler("x".into()),
            Self::BadArgs {
                target: "x".into(),
                why: "y".into(),
            },
            Self::IssuerMismatch { txn: t },
            Self::DuplicateCommand(CommandId(1)),
            Self::PolicyRefused { what: "x".into() },
            Self::Poisoned,
            Self::DuplicateHandler("x".into()),
        ]
    }
}

impl fmt::Display for CmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::UnknownEntity(e) => write!(f, "entity {e} does not exist"),
            Self::Cycle { entity, parent } => write!(
                f,
                "moving {entity} under {parent} would make it its own ancestor"
            ),
            Self::BadPath(p) => write!(f, "{p:?} is not a property path (ident(.ident)*)"),
            Self::NonFinite { path } => write!(f, "the value for {path:?} is NaN or infinite"),
            Self::UnknownTxn(t) => write!(f, "{t} does not exist on this bus"),
            Self::TxnState {
                txn,
                expected,
                found,
            } => write!(f, "{txn} is {found}, the operation needs it {expected}"),
            Self::BadName(n) => write!(
                f,
                "{n:?} is not an entity name (1-256 characters, no control characters)"
            ),
            Self::Conflict { detail } => {
                write!(f, "the project changed underneath the command: {detail}")
            }
            Self::Panicked { message } => write!(f, "the command panicked: {message}"),
            Self::UnknownHandler(t) => write!(f, "no command handler is registered for {t:?}"),
            Self::BadArgs { target, why } => write!(f, "bad arguments for {target:?}: {why}"),
            Self::IssuerMismatch { txn } => write!(
                f,
                "the command's issuer is not the issuer of {txn}; open your own transaction"
            ),
            Self::DuplicateCommand(c) => write!(
                f,
                "{c} was already applied, or is older than the replay window"
            ),
            Self::PolicyRefused { what } => write!(f, "refused by policy: {what}"),
            Self::Poisoned => write!(
                f,
                "the bus is poisoned by an earlier unrecoverable failure; reload the project"
            ),
            Self::DuplicateHandler(t) => write!(f, "a handler for {t:?} is already registered"),
        }
    }
}

impl std::error::Error for CmdError {}

impl CodedError for CmdError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}

/// The bus's refusal: which command (if one), which transaction, and why. A panel turns it
/// into a toast naming the command and the stable code (Ch.21.18); nothing is dropped
/// silently, and every rejection is also in the audit log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// The refused command, if the refusal was of a command (not of an undo or commit).
    pub command: Option<CommandId>,
    /// The transaction involved.
    pub txn: Option<TxnId>,
    /// Why.
    pub error: CmdError,
}

impl Rejection {
    /// A rejection of `error` for `command` in `txn`.
    #[must_use]
    pub fn new(error: CmdError, command: Option<CommandId>, txn: Option<TxnId>) -> Self {
        Self {
            command,
            txn,
            error,
        }
    }

    /// The stable code.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        self.error.code()
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)?;
        match (self.command, self.txn) {
            (Some(c), Some(t)) => write!(f, " [{c}, {t}]"),
            (Some(c), None) => write!(f, " [{c}]"),
            (None, Some(t)) => write!(f, " [{t}]"),
            (None, None) => Ok(()),
        }
    }
}

impl std::error::Error for Rejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl CodedError for Rejection {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
