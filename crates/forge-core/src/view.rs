//! `RegionView` — a region system's window onto the world (Ch.5.3).

use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::prelude::Mut;
use forge_frames::FrameId;

use crate::region::OwnerTable;
use crate::{ComponentKey, CoreError, EntityId, Profile, Region, RegionAccess, RegionId};

/// What a view may touch, component-wise.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Scope<'a> {
    /// A running system: exactly what it declared.
    Declared(&'a RegionAccess),
    /// A barrier effect: runs alone, so any component of the target region's entities.
    Barrier,
}

/// A region's view of the world: hands out `&` and `&mut` **only to entities the region
/// owns**, and only for components the running system declared.
///
/// Both checks are made on every access and fail with a coded error rather than a panic
/// (`CORE-0005` not owned, `CORE-0006` undeclared), so a buggy system can be reported and
/// the tick carries on. In this single-threaded skeleton the checks are what make the
/// scheduler's disjointness proof *true*: a system can only ever touch what the proof
/// assumed it touches. When the scheduler goes parallel (M4-1) the same view is handed
/// out per thread over the proven-disjoint entity sets.
pub struct RegionView<'w> {
    ecs: &'w mut bevy_ecs::world::World,
    region: &'w Region,
    /// The world's dense owner table: the per-access ownership check is `O(1)`.
    owners: &'w OwnerTable,
    scope: Scope<'w>,
}

impl<'w> RegionView<'w> {
    pub(crate) fn new(
        ecs: &'w mut bevy_ecs::world::World,
        region: &'w Region,
        owners: &'w OwnerTable,
        scope: Scope<'w>,
    ) -> Self {
        Self {
            ecs,
            region,
            owners,
            scope,
        }
    }

    /// The region this view belongs to.
    #[must_use]
    pub fn id(&self) -> RegionId {
        self.region.id()
    }

    /// The region's frame.
    #[must_use]
    pub fn frame(&self) -> FrameId {
        self.region.frame()
    }

    /// The region's profile. Use it to choose a cheaper algorithm, never a different outcome
    /// (I5).
    #[must_use]
    pub fn profile(&self) -> Profile {
        self.region.profile()
    }

    /// The owned entities, ascending.
    pub fn entities(&self) -> impl ExactSizeIterator<Item = EntityId> + '_ {
        self.region.entities()
    }

    /// Does this region own `e`? `O(1)`.
    #[inline]
    #[must_use]
    pub fn owns(&self, e: EntityId) -> bool {
        self.owners.is_owned_by(e, self.region.id())
    }

    fn check(&self, e: EntityId, k: &ComponentKey, write: bool) -> Result<(), CoreError> {
        if !self.owns(e) {
            return Err(CoreError::NotOwned {
                entity: e,
                region: self.region.id(),
            });
        }
        let allowed = match self.scope {
            Scope::Barrier => true,
            Scope::Declared(a) if write => a.can_write(k),
            Scope::Declared(a) => a.can_read(k),
        };
        if allowed {
            Ok(())
        } else {
            Err(CoreError::UndeclaredAccess {
                region: self.region.id(),
                component: k.name(),
                write,
            })
        }
    }

    /// Read component `C` of an owned entity.
    pub fn get<C: Component>(&self, e: EntityId) -> Result<&C, CoreError> {
        let k = ComponentKey::of::<C>();
        self.check(e, &k, false)?;
        self.ecs
            .get::<C>(e.entity())
            .ok_or(CoreError::MissingComponent {
                entity: e,
                component: k.name(),
            })
    }

    /// Does the owned entity `e` have `C`? (A read of `C`.)
    pub fn has<C: Component>(&self, e: EntityId) -> Result<bool, CoreError> {
        let k = ComponentKey::of::<C>();
        self.check(e, &k, false)?;
        Ok(self.ecs.get::<C>(e.entity()).is_some())
    }

    /// Mutate component `C` of an owned entity (change detection sees the write).
    pub fn get_mut<C: Component<Mutability = Mutable>>(
        &mut self,
        e: EntityId,
    ) -> Result<Mut<'_, C>, CoreError> {
        let k = ComponentKey::of::<C>();
        self.check(e, &k, true)?;
        self.ecs
            .get_mut::<C>(e.entity())
            .ok_or(CoreError::MissingComponent {
                entity: e,
                component: k.name(),
            })
    }

    /// Insert (or replace) component `C` on an owned entity. A write of `C`.
    pub fn insert<C: Component<Mutability = Mutable>>(
        &mut self,
        e: EntityId,
        value: C,
    ) -> Result<(), CoreError> {
        let k = ComponentKey::of::<C>();
        self.check(e, &k, true)?;
        let mut ent = self
            .ecs
            .get_entity_mut(e.entity())
            .map_err(|_| CoreError::UnknownEntity(e))?;
        ent.insert(value);
        Ok(())
    }
}
