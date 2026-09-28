//! `forge-core` — the error contract, profiles, and the ECS core with its regionized
//! scheduler (Ch.1.2, Ch.1.4, Ch.5).
//!
//! * [`Error`] / [`ErrorCode`] / [`CodedError`] — the one engine-wide error, every variant
//!   of every crate's enum carrying a stable code from `docs/error-codes.md`.
//! * [`Profile`] and [`ProfileSelector`] — a region's cost profile, chosen by measurement with
//!   hysteresis.
//! * [`World`] — a `bevy_ecs` world used as a library (Ch.5.1), plus the region table.
//! * [`Region`], [`RegionId`], [`Regions`] — disjoint entity ownership.
//! * [`RegionSystem`], [`RegionAccess`], [`RegionView`], [`Outbox`], [`Scheduler`] — the
//!   regionized scheduler: plan, prove disjointness, run, barrier. Single-threaded in this
//!   skeleton (M0-9); the proof it runs is the one the parallel version (M4-1) relies on.
//!
//! `bevy_ecs` is re-exported so downstream crates derive `Component`/`Bundle` against the
//! exact pinned version (Risk S6: one pin, one place).

pub use bevy_ecs;

mod access;
mod core_error;
mod entity;
mod error;
mod outbox;
mod profile;
mod region;
mod scheduler;
mod view;
mod world;

pub use access::{ComponentKey, RegionAccess};
pub use core_error::CoreError;
pub use entity::EntityId;
pub use error::{CodedError, Ctx, CtxValue, Error, ErrorCode, ErrorInner, Result, ResultExt};
pub use outbox::Outbox;
pub use profile::{Band, Profile, ProfileMetrics, ProfileSelector, ProfileThresholds};
pub use region::{OwnerTable, Region, RegionId, Regions};
pub use scheduler::{Job, Plan, RegionSystem, Scheduler, SystemIndex, TickReport, Wave};
pub use view::RegionView;
pub use world::World;

/// True in the `mutate-disjointness` build (the W2 positive-control mutant, where `split`
/// leaves moved entities in the source region too). The disjointness test's control uses
/// it to avoid recursing into itself.
pub const MUTATE_DISJOINTNESS: bool = cfg!(feature = "mutate-disjointness");
