//! `forge-cmd` — the command bus, transactions, undo and provenance (Ch.7).
//!
//! **There is exactly one way to change project state (I7).** The UI, automation sessions, scripts,
//! tests and the CLI are all clients that send [`CommandEnvelope`]s to a [`CommandSink`]. The
//! in-process sink is the [`Bus`], which owns the [`Project`]; nothing outside this crate can reach
//! a `&mut Project`.
//!
//! * A command is planned into a [`Diff`] by a [`DiffBuilder`] over a **read-only** project.
//!   Every [`Change`] carries its `before` and `after`, so every diff is invertible: undo and
//!   redo need no per-command code (I8 by construction), and `dry_run` returns exactly the
//!   diff `apply` will apply (the split editor's optimistic preview, O-13).
//! * `apply` is atomic: every change checks that its `before` still matches, and a failure
//!   rolls back what was applied. A panic in a command is caught at the bus boundary and
//!   becomes a [`Rejection`] (`CMD-0009`); the editor stays up (Ch.1.2).
//! * Transactions: an envelope with a fresh [`TxnId`] is one undo step. A gesture opens one
//!   with [`Bus::begin`], sends a command per frame into it (value changes **merge**: a drag
//!   of any length is one change per property, with the value from before the drag), and
//!   [`Bus::commit`]s on release or [`Bus::cancel`]s on Esc.
//! * Provenance: every envelope carries its [`Issuer`]; every history entry, [`Applied`]
//!   event and [`AuditEntry`] (including every refusal) records it.
//! * The [`Applied`] stream ([`Bus::subscribe`]) is how the UI, the split editor and the
//!   collab layer observe state, for every issuer alike. Each subscription is bounded: a
//!   stalled subscriber gets a [`Gap`] marker and resyncs from the latest state instead of
//!   growing memory without limit.
//!
//! ```
//! use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer, Value};
//!
//! let mut bus = Bus::new();
//! let feed = bus.subscribe();
//! let me = Issuer::Human { user: "ada".into() };
//!
//! let spawn = bus.envelope(me.clone(), EditorCommand::Spawn { name: "Cube".into(), parent: None });
//! let cube = match &bus.apply(spawn).expect("applies").diff.changes[0] {
//!     forge_cmd::Change::Created { entity, .. } => *entity,
//!     _ => unreachable!(),
//! };
//!
//! // A slider drag: one transaction, a command per frame, one undo step.
//! let drag = bus.begin("Drag scale", me.clone());
//! for x in [1.0, 1.5, 2.0] {
//!     let set = EditorCommand::SetProperty { entity: cube, path: "scale".into(), value: Value::Float(x) };
//!     let e = bus.envelope_in(drag, me.clone(), set);
//!     bus.apply(e).expect("applies");
//! }
//! bus.commit(drag).expect("commits");
//! assert_eq!(bus.transaction(drag).map(|t| t.changes().count()), Some(1));
//!
//! bus.undo(drag).expect("undoes the whole drag");
//! assert_eq!(bus.project().entity(cube).and_then(|e| e.property("scale")), None);
//! assert_eq!(feed.drain().len(), 1 + 3 + 1 + 1); // spawn, 3 frames, commit, undo
//! ```

#![forbid(unsafe_code)]

mod audit;
mod builder;
mod bus;
mod command;
pub mod contract;
mod diff;
mod error;
mod ids;
mod project;
mod replica;
mod stream;
mod value;

pub use audit::{AuditAction, AuditEntry, Clock, FixedClock, SystemClock};
pub use builder::{DiffBuilder, MAX_NAME_CHARS, check_name, check_path};
pub use bus::{
    Applied, AppliedKind, Bus, CommandDeriver, CommandGuard, CommandHandler, CommandSink,
    CommandSpan, DEFAULT_MAX_AUDIT, DEFAULT_MAX_HISTORY, Footprint, Guarded, REPLAY_WINDOW,
    RETIRED_WINDOW, TxnRecord,
};
pub use command::{CommandEnvelope, CommandPolicy, EditorCommand};
pub use diff::{Change, Diff};
pub use error::{CmdError, Rejection, TxnState};
pub use ids::{CommandId, EntityKey, Issuer, TxnId};
pub use project::{EntityData, Project};
pub use replica::Replica;
pub use stream::{DEFAULT_SUBSCRIPTION_CAPACITY, Drained, Gap, StreamItem, Subscription};
pub use value::Value;

/// True in the `mutate-undo` build (the W2 positive-control mutant, where transaction
/// merging keeps the newest `before`). The undo fuzz's control uses it to avoid recursing.
pub const MUTATE_UNDO: bool = cfg!(feature = "mutate-undo");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cmd_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in CmdError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-cmd |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(
                e.to_string().starts_with(code.as_str()),
                "Display leads with the code"
            );
        }
    }

    #[test]
    fn rejection_converts_into_the_engine_error() {
        fn f() -> forge_core::Result<()> {
            Err(Rejection::new(CmdError::Poisoned, None, None))?;
            Ok(())
        }
        assert_eq!(f().expect_err("fails").code().as_str(), "CMD-0015");
    }
}
