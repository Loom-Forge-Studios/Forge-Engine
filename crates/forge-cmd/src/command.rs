//! [`EditorCommand`] and [`CommandEnvelope`] — what travels on the bus.

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::{CommandId, DiffBuilder, EntityKey, Issuer, TxnId, Value};

/// Every way project state can change (I7). `Reflect` (the inspector and the API schema
/// see it), `Serialize`/`Deserialize` (the command log, the remote wire, the audit trail).
///
/// Each variant is planned into a [`Diff`](crate::Diff) by [`EditorCommand::plan`] against a
/// read-only project; the bus applies that diff. There is no other code path, which is why
/// every variant is dry-runnable and undoable by construction (I8, proven per variant by
/// `tests/liveness/test_command_contract.rs`).
///
/// **Appending a variant** means: plan it in [`EditorCommand::plan`], give it a
/// [`CommandPolicy`] if it is not an ordinary undoable command, and add a sample to
/// [`crate::contract::samples`] — the contract guard fails until the sample exists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub enum EditorCommand {
    /// Create an entity (its key is allocated by the core and reported in the diff).
    Spawn {
        /// Its name.
        name: String,
        /// Its parent (`None`: a root).
        parent: Option<EntityKey>,
    },
    /// Remove an entity and its whole subtree.
    Despawn {
        /// The entity.
        entity: EntityKey,
    },
    /// Rename an entity.
    Rename {
        /// The entity.
        entity: EntityKey,
        /// The new name.
        name: String,
    },
    /// Move an entity in the hierarchy.
    Reparent {
        /// The entity.
        entity: EntityKey,
        /// The new parent (`None`: make it a root).
        parent: Option<EntityKey>,
    },
    /// Set a property (reflect path) on an entity.
    SetProperty {
        /// The entity.
        entity: EntityKey,
        /// The reflect path (`transform.position`).
        path: String,
        /// The value.
        value: Value,
    },
    /// Remove a property from an entity.
    RemoveProperty {
        /// The entity.
        entity: EntityKey,
        /// The reflect path.
        path: String,
    },
    /// Set (`Some`) or clear (`None`) a project setting.
    SetSetting {
        /// The key (`ident(.ident)*`).
        key: String,
        /// The value.
        value: Option<Value>,
    },
    /// Invoke a registered command handler: a mutating `#[forge_api]` fn (Ch.6 output 4) or
    /// a plugin command (Ch.32). The handler plans a diff like any built-in; it never sees
    /// `&mut` project state.
    Invoke {
        /// The handler's name (a `CommandDesc::target`, or a plugin's command id).
        target: String,
        /// The arguments, as a JSON object (text, so the command stays `Reflect`).
        args: String,
    },
}

/// What undo/redo and issuers a command allows. Every current variant is an ordinary
/// command (undoable, redoable, any issuer); security-class commands (WP-U9: plugin and
/// automation grants) set the other values — "undo and redo never grant" (Ch.21.18).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CommandPolicy {
    /// Only `Issuer::Human` may send it.
    pub human_only: bool,
    /// Its transaction may be undone.
    pub undoable: bool,
    /// Its transaction may be redone after an undo.
    pub redoable: bool,
}

impl CommandPolicy {
    /// An ordinary command.
    pub const ORDINARY: Self = Self {
        human_only: false,
        undoable: true,
        redoable: true,
    };
}

impl EditorCommand {
    /// The variant's name (`"SetProperty"`).
    #[must_use]
    pub fn variant(&self) -> &'static str {
        match self {
            Self::Spawn { .. } => "Spawn",
            Self::Despawn { .. } => "Despawn",
            Self::Rename { .. } => "Rename",
            Self::Reparent { .. } => "Reparent",
            Self::SetProperty { .. } => "SetProperty",
            Self::RemoveProperty { .. } => "RemoveProperty",
            Self::SetSetting { .. } => "SetSetting",
            Self::Invoke { .. } => "Invoke",
        }
    }

    /// A short human label for the undo-history panel and toasts.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Spawn { name, .. } => format!("Spawn {name}"),
            Self::Despawn { entity } => format!("Delete {entity}"),
            Self::Rename { entity, name } => format!("Rename {entity} to {name}"),
            Self::Reparent { entity, .. } => format!("Move {entity}"),
            Self::SetProperty { entity, path, .. } => format!("Set {entity}.{path}"),
            Self::RemoveProperty { entity, path } => format!("Remove {entity}.{path}"),
            Self::SetSetting { key, .. } => format!("Set setting {key}"),
            Self::Invoke { target, .. } => target.clone(),
        }
    }

    /// Its policy.
    #[must_use]
    pub fn policy(&self) -> CommandPolicy {
        CommandPolicy::ORDINARY
    }

    /// Plan a built-in variant into `b`. `Invoke` is planned by the bus (it needs the handler
    /// table) and is refused here.
    pub(crate) fn plan_builtin(&self, b: &mut DiffBuilder<'_>) -> Result<(), crate::CmdError> {
        match self {
            Self::Spawn { name, parent } => b.spawn(name, *parent).map(|_| ()),
            Self::Despawn { entity } => b.despawn(*entity),
            Self::Rename { entity, name } => b.rename(*entity, name),
            Self::Reparent { entity, parent } => b.reparent(*entity, *parent),
            Self::SetProperty {
                entity,
                path,
                value,
            } => b.set_property(*entity, path, value.clone()),
            Self::RemoveProperty { entity, path } => b.remove_property(*entity, path),
            Self::SetSetting { key, value } => b.set_setting(key, value.clone()),
            Self::Invoke { target, .. } => Err(crate::CmdError::UnknownHandler(target.clone())),
        }
    }
}

/// A command on the wire: who sent it, in which transaction, under which id (Ch.7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub struct CommandEnvelope {
    /// Unique per bus; a resend with the same id is refused, never applied twice.
    pub id: CommandId,
    /// The transaction: an open one (a gesture) or a fresh id (one command, one undo step).
    pub txn: TxnId,
    /// Provenance.
    pub issuer: Issuer,
    /// The command.
    pub cmd: EditorCommand,
}
