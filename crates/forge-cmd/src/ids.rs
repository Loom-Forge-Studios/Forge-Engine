//! Identities on the bus: [`CommandId`], [`TxnId`], [`EntityKey`] and the [`Issuer`].

use std::fmt;

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// One command, unique for the life of a bus. The bus refuses a second envelope with an id
/// it has already applied (`CMD-0013`), so a resent network packet never applies twice. The
/// bus allocates ids in increasing order and remembers the last `REPLAY_WINDOW` exactly; an
/// id older than that window is refused as a stale resend.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, Reflect,
)]
pub struct CommandId(pub u64);

/// A transaction: the unit of undo. One user intent (a click, a whole slider drag, an
/// automation session's approved batch) is one transaction and one undo step.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, Reflect,
)]
pub struct TxnId(pub u64);

/// A project entity's **stable** key: allocated by the core, never reused (an undone spawn
/// does not give its key back), identical in every session and on every client, so the
/// command log (Ch.33) and a remote mirror (Ch.34) can name it.
///
/// This is deliberately not `forge_core::EntityId`, which is a per-session `bevy_ecs`
/// handle whose bits are only meaningful inside one `World` (ADR 0009).
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, Reflect,
)]
pub struct EntityKey(pub u64);

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cmd#{}", self.0)
    }
}

impl fmt::Display for TxnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "txn#{}", self.0)
    }
}

impl fmt::Display for EntityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}", self.0)
    }
}

/// Who issued a command — the provenance tag on every envelope, history entry, audit line
/// and `Applied` event. The editor shell stamps `Human`; panels cannot forge it (Ch.21.18).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Reflect)]
pub enum Issuer {
    /// A person at an editor.
    Human {
        /// The user's name or id.
        user: String,
    },
    /// An automation session: a non-human client a plugin hosts over the core (a protocol
    /// server's session). Refused every security-class command, like a script.
    Automation {
        /// The session's id (`auto-3`).
        session: String,
        /// What the session used to send the command (a tool or request name).
        tool: String,
    },
    /// A script or CLI invocation.
    Script {
        /// The script's path.
        path: String,
    },
    /// A test.
    Test,
}

impl Issuer {
    /// The short provenance tag the undo-history panel shows: `human:alice`,
    /// `automation:<session>`, `script:<path>`, `test`.
    #[must_use]
    pub fn tag(&self) -> String {
        match self {
            Self::Human { user } => format!("human:{user}"),
            Self::Automation { session, .. } => format!("automation:{session}"),
            Self::Script { path } => format!("script:{path}"),
            Self::Test => "test".to_string(),
        }
    }

    /// True for [`Issuer::Human`] — the only issuer a security-class command accepts.
    #[must_use]
    pub fn is_human(&self) -> bool {
        matches!(self, Self::Human { .. })
    }
}

impl fmt::Display for Issuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.tag())
    }
}
