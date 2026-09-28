//! `EntityId` — the engine's entity handle (Ch.1.3).

use std::fmt;

use bevy_ecs::entity::Entity;

/// An entity in a [`crate::World`]: a `bevy_ecs` [`Entity`] (index + generation) behind a
/// Forge type, so no public signature above `forge-core` names a `bevy_ecs` type for it and a
/// `bevy_ecs` upgrade (Risk S6) stops here.
///
/// Ordered by `(index, generation)`, which gives regions and the scheduler a total,
/// platform-independent order to iterate in (determinism, Ch.3).
///
/// Reflected **opaque** (Ch.6): tools see an entity handle, never its bit layout.
#[derive(Clone, Copy, PartialEq, Eq, Hash, bevy_reflect::Reflect)]
#[reflect(opaque, Clone, Debug, PartialEq, Hash)]
pub struct EntityId(Entity);

impl EntityId {
    /// Wrap a `bevy_ecs` entity.
    #[must_use]
    pub const fn from_entity(e: Entity) -> Self {
        Self(e)
    }

    /// The underlying `bevy_ecs` entity.
    #[must_use]
    pub const fn entity(self) -> Entity {
        self.0
    }

    /// Stable 64-bit form (index in the low bits, generation in the high bits), suitable for
    /// serialisation within one session.
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        self.0.to_bits()
    }

    /// Inverse of [`EntityId::to_bits`]; `None` for bit patterns that are not an entity.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Option<Self> {
        match Entity::try_from_bits(bits) {
            Some(e) => Some(Self(e)),
            None => None,
        }
    }

    /// The slot index (reused after despawn, with a new generation).
    #[must_use]
    pub fn index(self) -> u32 {
        self.0.index_u32()
    }
}

impl EntityId {
    /// `(index, generation)` packed so integer order is slot order. (`to_bits` is not: bevy
    /// stores the index bit-inverted, so fresh entities would sort backwards.)
    fn order_key(self) -> u64 {
        (u64::from(self.0.index_u32()) << 32) | u64::from(self.0.generation().to_bits())
    }
}

impl PartialOrd for EntityId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EntityId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order_key().cmp(&other.order_key())
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({})", self.0)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_round_trip() {
        let mut w = bevy_ecs::world::World::new();
        let a = EntityId::from_entity(w.spawn_empty().id());
        let b = EntityId::from_entity(w.spawn_empty().id());
        assert_eq!(EntityId::from_bits(a.to_bits()), Some(a));
        assert_ne!(a, b);
        assert!(a < b);
        assert_eq!(a.index() + 1, b.index());
    }
}
