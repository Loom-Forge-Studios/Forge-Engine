//! `Outbox` — a region's cross-region effects, applied at the barrier (Ch.5.3).

use bevy_ecs::bundle::Bundle;
use bevy_ecs::entity::Entity;

use crate::{EntityId, Error, RegionId, RegionView};

/// A barrier effect on another region's entities.
pub(crate) type Effect = Box<dyn FnOnce(&mut RegionView<'_>) -> Result<(), Error> + Send>;
/// A deferred spawn.
pub(crate) type Spawner = Box<dyn FnOnce(&mut bevy_ecs::world::World) -> Entity + Send>;

/// One queued cross-region message.
pub(crate) enum Message {
    Effect { to: RegionId, f: Effect },
    Transfer { entity: EntityId, to: RegionId },
    Spawn { to: RegionId, f: Spawner },
    Despawn { entity: EntityId },
}

/// Where a region system puts everything that reaches outside its own region.
///
/// A system may mutate only its own region's entities, directly. Anything else — touching
/// another region, handing an entity over, spawning, despawning — is queued here and applied
/// at the **barrier** after every wave of the tick has run, in a fixed order (by sending
/// region, then system, then send order), so the result does not depend on how the waves
/// were packed or, later, on which thread finished first.
///
/// Messages are validated when applied, against the world as it is at the barrier: a
/// transfer or despawn of an entity the sender no longer owns, or a message to a region that
/// no longer exists, is rejected with a coded error in the tick report — never a panic.
pub struct Outbox {
    from: RegionId,
    pub(crate) msgs: Vec<Message>,
}

impl Outbox {
    pub(crate) fn new(from: RegionId) -> Self {
        Self {
            from,
            msgs: Vec::new(),
        }
    }

    /// The region this outbox belongs to.
    #[must_use]
    pub fn from(&self) -> RegionId {
        self.from
    }

    /// Number of queued messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.msgs.len()
    }

    /// Nothing queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.msgs.is_empty()
    }

    /// Run `f` against region `to` at the barrier, with unrestricted component access to that
    /// region's entities (and still none to anyone else's).
    pub fn send(
        &mut self,
        to: RegionId,
        f: impl FnOnce(&mut RegionView<'_>) -> Result<(), Error> + Send + 'static,
    ) {
        self.msgs.push(Message::Effect { to, f: Box::new(f) });
    }

    /// Hand an entity this region owns to region `to` at the barrier.
    pub fn transfer(&mut self, entity: EntityId, to: RegionId) {
        self.msgs.push(Message::Transfer { entity, to });
    }

    /// Spawn `bundle` at the barrier, owned by region `to`.
    pub fn spawn_in<B: Bundle>(&mut self, to: RegionId, bundle: B) {
        self.msgs.push(Message::Spawn {
            to,
            f: Box::new(move |w| w.spawn(bundle).id()),
        });
    }

    /// Despawn an entity this region owns, at the barrier.
    pub fn despawn(&mut self, entity: EntityId) {
        self.msgs.push(Message::Despawn { entity });
    }
}
