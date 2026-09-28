//! `CoreError` — forge-core's own error enum (`CORE-*` codes, `docs/error-codes.md`).

use std::fmt;

use forge_frames::FrameId;

use crate::{CodedError, EntityId, ErrorCode, RegionId, error_code};

/// Every way a forge-core operation can fail. Converts into [`crate::Error`] with `?`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    /// `CORE-0001`: the entity does not exist in this world (never spawned, or despawned).
    UnknownEntity(EntityId),
    /// `CORE-0002`: no region has this id (never created, merged away, or removed).
    UnknownRegion(RegionId),
    /// `CORE-0003`: the entity is already owned by another region; ownership moves only by
    /// an explicit transfer, never by a second assignment.
    AlreadyOwned {
        /// The entity.
        entity: EntityId,
        /// Its current owner.
        owner: RegionId,
    },
    /// `CORE-0004`: two regions in different frames cannot merge (Ch.5.3: "two regions may
    /// only merge if they share a frame").
    FrameMismatch {
        /// The surviving region.
        into: RegionId,
        /// The region that would have been absorbed.
        from: RegionId,
        /// `into`'s frame.
        into_frame: FrameId,
        /// `from`'s frame.
        from_frame: FrameId,
    },
    /// `CORE-0005`: the region does not own the entity, so it may not read, write, split
    /// off, transfer or despawn it.
    NotOwned {
        /// The entity.
        entity: EntityId,
        /// The region that asked.
        region: RegionId,
    },
    /// `CORE-0006`: a region system touched a component its `RegionAccess` does not
    /// declare — the scheduler's disjointness proof relies on the declaration being true.
    UndeclaredAccess {
        /// The region the system was running in.
        region: RegionId,
        /// The component's type name.
        component: &'static str,
        /// Whether the undeclared access was a write.
        write: bool,
    },
    /// `CORE-0007`: disjointness proof failed — the same entity is in two regions that
    /// would run concurrently.
    Overlap {
        /// The entity held by both.
        entity: EntityId,
        /// One region.
        a: RegionId,
        /// The other.
        b: RegionId,
    },
    /// `CORE-0008`: disjointness proof failed — two systems in one wave, on the same
    /// region, conflict on a component (one writes what the other reads or writes).
    AccessConflict {
        /// One system (by name).
        a: String,
        /// The other.
        b: String,
        /// The component they conflict on.
        component: &'static str,
    },
    /// `CORE-0009`: every `RegionId` has been used.
    RegionsExhausted,
    /// `CORE-0010`: the entity does not have the requested component.
    MissingComponent {
        /// The entity.
        entity: EntityId,
        /// The component's type name.
        component: &'static str,
    },
    /// `CORE-0011`: a plan names a system index the scheduler does not have.
    UnknownSystem(usize),
    /// `CORE-0012`: canonical time would overflow `i64` ticks.
    TickOverflow,
}

impl CoreError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::UnknownEntity(_) => error_code!("CORE-0001"),
            Self::UnknownRegion(_) => error_code!("CORE-0002"),
            Self::AlreadyOwned { .. } => error_code!("CORE-0003"),
            Self::FrameMismatch { .. } => error_code!("CORE-0004"),
            Self::NotOwned { .. } => error_code!("CORE-0005"),
            Self::UndeclaredAccess { .. } => error_code!("CORE-0006"),
            Self::Overlap { .. } => error_code!("CORE-0007"),
            Self::AccessConflict { .. } => error_code!("CORE-0008"),
            Self::RegionsExhausted => error_code!("CORE-0009"),
            Self::MissingComponent { .. } => error_code!("CORE-0010"),
            Self::UnknownSystem(_) => error_code!("CORE-0011"),
            Self::TickOverflow => error_code!("CORE-0012"),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<CoreError> {
        let e = EntityId::from_entity(bevy_ecs::entity::Entity::PLACEHOLDER);
        let r = RegionId(0);
        vec![
            Self::UnknownEntity(e),
            Self::UnknownRegion(r),
            Self::AlreadyOwned {
                entity: e,
                owner: r,
            },
            Self::FrameMismatch {
                into: r,
                from: r,
                into_frame: FrameId(0),
                from_frame: FrameId(1),
            },
            Self::NotOwned {
                entity: e,
                region: r,
            },
            Self::UndeclaredAccess {
                region: r,
                component: "C",
                write: true,
            },
            Self::Overlap {
                entity: e,
                a: r,
                b: r,
            },
            Self::AccessConflict {
                a: "a".into(),
                b: "b".into(),
                component: "C",
            },
            Self::RegionsExhausted,
            Self::MissingComponent {
                entity: e,
                component: "C",
            },
            Self::UnknownSystem(0),
            Self::TickOverflow,
        ]
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::UnknownEntity(e) => write!(f, "entity {e} does not exist"),
            Self::UnknownRegion(r) => write!(f, "region {} does not exist", r.0),
            Self::AlreadyOwned { entity, owner } => write!(
                f,
                "entity {entity} is already owned by region {} (transfer it instead)",
                owner.0
            ),
            Self::FrameMismatch {
                into,
                from,
                into_frame,
                from_frame,
            } => write!(
                f,
                "region {} (frame {}) cannot merge into region {} (frame {}): regions merge only \
                 within one frame",
                from.0, from_frame.0, into.0, into_frame.0
            ),
            Self::NotOwned { entity, region } => {
                write!(f, "entity {entity} is not owned by region {}", region.0)
            }
            Self::UndeclaredAccess {
                region,
                component,
                write,
            } => write!(
                f,
                "a system in region {} {} `{component}` without declaring it in its RegionAccess",
                region.0,
                if *write { "wrote" } else { "read" }
            ),
            Self::Overlap { entity, a, b } => write!(
                f,
                "entity {entity} is held by both region {} and region {} — they cannot run \
                 concurrently",
                a.0, b.0
            ),
            Self::AccessConflict { a, b, component } => write!(
                f,
                "systems `{a}` and `{b}` conflict on `{component}` and cannot share a wave"
            ),
            Self::RegionsExhausted => write!(f, "no RegionId is left"),
            Self::MissingComponent { entity, component } => {
                write!(f, "entity {entity} has no `{component}`")
            }
            Self::UnknownSystem(i) => write!(f, "the scheduler has no system #{i}"),
            Self::TickOverflow => write!(f, "canonical time overflowed"),
        }
    }
}

impl std::error::Error for CoreError {}

impl CodedError for CoreError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
