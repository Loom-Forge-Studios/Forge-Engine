//! `World` — a `bevy_ecs` world plus the region ownership table (Ch.5).

use bevy_ecs::bundle::Bundle;
use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::prelude::Mut;
use forge_frames::{FrameId, Tick};

use crate::region::OwnerTable;
use crate::view::Scope;
use crate::{
    ComponentKey, CoreError, EntityId, Profile, ProfileMetrics, Region, RegionAccess, RegionId,
    RegionView, Regions,
};

/// One simulation world: entities and components (a `bevy_ecs::World`, used as a library —
/// Ch.5.1), the regions that own them, and canonical time.
///
/// Worlds are cheap (Ch.5.2): an empty one is a few kilobytes, so fifty solo players are
/// fifty worlds, not a packing problem.
///
/// Outside a scheduler tick, `World` is the direct access path (setup, tests, and — once
/// `forge-cmd` lands — the command handlers that are the only mutators of project state,
/// I7). Inside a tick, systems see only a [`RegionView`].
#[derive(Default)]
pub struct World {
    ecs: bevy_ecs::world::World,
    regions: Regions,
    tick: Tick,
}

impl World {
    /// An empty world at tick 0.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only access to the underlying `bevy_ecs` world (queries, diagnostics). There is
    /// deliberately no `&mut` accessor: every mutation goes through a path that keeps the
    /// region table true.
    #[must_use]
    pub fn ecs(&self) -> &bevy_ecs::world::World {
        &self.ecs
    }

    /// The region ownership table.
    #[must_use]
    pub fn regions(&self) -> &Regions {
        &self.regions
    }

    /// Canonical time: the number of scheduler ticks run.
    #[must_use]
    pub fn tick(&self) -> Tick {
        self.tick
    }

    pub(crate) fn advance_tick(&mut self) -> Result<(), CoreError> {
        self.tick = self.tick.checked_add(1).ok_or(CoreError::TickOverflow)?;
        Ok(())
    }

    // ---- entities ------------------------------------------------------------------------

    /// Spawn an unowned entity.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> EntityId {
        EntityId::from_entity(self.ecs.spawn(bundle).id())
    }

    /// Spawn an entity owned by `region`. Checks the region first, so a bad id spawns nothing.
    pub fn spawn_in<B: Bundle>(
        &mut self,
        region: RegionId,
        bundle: B,
    ) -> Result<EntityId, CoreError> {
        self.regions.get(region)?;
        let e = self.spawn(bundle);
        self.regions.assign(e, region)?;
        Ok(e)
    }

    /// Does the entity exist?
    #[must_use]
    pub fn contains(&self, e: EntityId) -> bool {
        self.ecs.get_entity(e.entity()).is_ok()
    }

    fn require(&self, e: EntityId) -> Result<(), CoreError> {
        if self.contains(e) {
            Ok(())
        } else {
            Err(CoreError::UnknownEntity(e))
        }
    }

    /// Despawn an entity, releasing it from its region.
    pub fn despawn(&mut self, e: EntityId) -> Result<(), CoreError> {
        self.require(e)?;
        self.regions.release(e);
        self.ecs.despawn(e.entity());
        Ok(())
    }

    /// Insert (or replace) components on an entity.
    pub fn insert<B: Bundle>(&mut self, e: EntityId, bundle: B) -> Result<(), CoreError> {
        let mut ent = self
            .ecs
            .get_entity_mut(e.entity())
            .map_err(|_| CoreError::UnknownEntity(e))?;
        ent.insert(bundle);
        Ok(())
    }

    /// Read a component.
    pub fn get<C: Component>(&self, e: EntityId) -> Result<&C, CoreError> {
        self.require(e)?;
        self.ecs
            .get::<C>(e.entity())
            .ok_or(CoreError::MissingComponent {
                entity: e,
                component: ComponentKey::of::<C>().name(),
            })
    }

    /// Mutate a component.
    pub fn get_mut<C: Component<Mutability = Mutable>>(
        &mut self,
        e: EntityId,
    ) -> Result<Mut<'_, C>, CoreError> {
        self.require(e)?;
        self.ecs
            .get_mut::<C>(e.entity())
            .ok_or(CoreError::MissingComponent {
                entity: e,
                component: ComponentKey::of::<C>().name(),
            })
    }

    // ---- regions -------------------------------------------------------------------------

    /// Create an empty region.
    pub fn create_region(
        &mut self,
        frame: FrameId,
        profile: Profile,
    ) -> Result<RegionId, CoreError> {
        self.regions.create(frame, profile)
    }

    /// The region with this id.
    pub fn region(&self, id: RegionId) -> Result<&Region, CoreError> {
        self.regions.get(id)
    }

    /// Give an unowned entity to a region (`CORE-0003` if another region owns it).
    pub fn assign(&mut self, e: EntityId, region: RegionId) -> Result<(), CoreError> {
        self.require(e)?;
        self.regions.assign(e, region)
    }

    /// Move an entity to a region, whoever owns it now.
    pub fn transfer(&mut self, e: EntityId, to: RegionId) -> Result<(), CoreError> {
        self.require(e)?;
        self.regions.transfer(e, to)
    }

    /// Make an entity unowned; returns its former owner.
    pub fn release(&mut self, e: EntityId) -> Option<RegionId> {
        self.regions.release(e)
    }

    /// Absorb `from` into `into` (same frame only, `CORE-0004`).
    pub fn merge_regions(&mut self, into: RegionId, from: RegionId) -> Result<RegionId, CoreError> {
        self.regions.merge(into, from)
    }

    /// Split `entities` off `region` into a new region.
    pub fn split_region(
        &mut self,
        region: RegionId,
        entities: &[EntityId],
    ) -> Result<RegionId, CoreError> {
        self.regions.split(region, entities)
    }

    /// Delete a region; its entities stay alive, unowned.
    pub fn remove_region(&mut self, id: RegionId) -> Result<Vec<EntityId>, CoreError> {
        self.regions.remove(id)
    }

    /// Feed a measurement to a region's profile selector (Ch.1.4).
    pub fn observe_profile(
        &mut self,
        id: RegionId,
        m: &ProfileMetrics,
    ) -> Result<Profile, CoreError> {
        self.regions.observe_profile(id, m)
    }

    /// Run `f` with a view of `region` restricted to the components `access` declares —
    /// exactly what a scheduled system gets. For tools and tests that act as one system.
    pub fn with_view<R>(
        &mut self,
        region: RegionId,
        access: &RegionAccess,
        f: impl FnOnce(&mut RegionView<'_>) -> R,
    ) -> Result<R, CoreError> {
        let (ecs, r, owners) = self.split_for(region)?;
        let mut view = RegionView::new(ecs, r, owners, Scope::Declared(access));
        Ok(f(&mut view))
    }

    /// Disjoint borrows of the ECS world, one region and the owner table — what a
    /// `RegionView` holds.
    pub(crate) fn split_for(
        &mut self,
        region: RegionId,
    ) -> Result<(&mut bevy_ecs::world::World, &Region, &OwnerTable), CoreError> {
        let r = self.regions.get(region)?;
        Ok((&mut self.ecs, r, self.regions.owners()))
    }

    pub(crate) fn regions_mut(&mut self) -> &mut Regions {
        &mut self.regions
    }

    pub(crate) fn ecs_mut(&mut self) -> &mut bevy_ecs::world::World {
        &mut self.ecs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[derive(Component, Debug, PartialEq)]
    struct Pos(i64);
    #[derive(Component, Debug, PartialEq)]
    struct Vel(i64);

    #[test]
    fn spawn_get_mutate_despawn() {
        let mut w = World::new();
        let e = w.spawn((Pos(1), Vel(2)));
        assert_eq!(w.get::<Pos>(e).ok(), Some(&Pos(1)));
        w.get_mut::<Pos>(e).map(|mut p| p.0 = 5).ok();
        assert_eq!(w.get::<Pos>(e).ok(), Some(&Pos(5)));
        w.despawn(e).ok();
        assert!(!w.contains(e));
        assert_eq!(w.get::<Pos>(e).err(), Some(CoreError::UnknownEntity(e)));
        let err: Error = w.despawn(e).err().map(Into::into).expect("must fail");
        assert_eq!(err.code().as_str(), "CORE-0001");
    }

    #[test]
    fn despawn_releases_ownership() {
        let mut w = World::new();
        let r = w
            .create_region(FrameId(0), Profile::Surface)
            .expect("region");
        let e = w.spawn_in(r, Pos(0)).expect("spawn");
        assert_eq!(w.regions().owner_of(e), Some(r));
        w.despawn(e).expect("despawn");
        assert_eq!(w.regions().owner_of(e), None);
        assert!(w.region(r).expect("region").is_empty());
        w.regions().check_invariants().expect("invariants");
    }

    #[test]
    fn spawn_in_a_missing_region_spawns_nothing() {
        let mut w = World::new();
        let before = w.ecs().entities().len();
        assert_eq!(
            w.spawn_in(RegionId(9), Pos(0)).err(),
            Some(CoreError::UnknownRegion(RegionId(9)))
        );
        assert_eq!(w.ecs().entities().len(), before);
    }

    #[test]
    fn ownership_moves_only_by_transfer() {
        let mut w = World::new();
        let a = w.create_region(FrameId(0), Profile::Surface).expect("a");
        let b = w.create_region(FrameId(0), Profile::Surface).expect("b");
        let e = w.spawn_in(a, Pos(0)).expect("spawn");
        assert_eq!(
            w.assign(e, b).err(),
            Some(CoreError::AlreadyOwned {
                entity: e,
                owner: a
            })
        );
        w.assign(e, a)
            .expect("re-assigning to the owner is a no-op");
        w.transfer(e, b).expect("transfer");
        assert_eq!(w.regions().owner_of(e), Some(b));
        assert!(!w.region(a).expect("a").owns(e));
        w.regions().check_invariants().expect("invariants");
    }

    #[test]
    fn merge_requires_one_frame() {
        let mut w = World::new();
        let a = w.create_region(FrameId(0), Profile::Surface).expect("a");
        let b = w.create_region(FrameId(1), Profile::Surface).expect("b");
        let c = w.create_region(FrameId(0), Profile::Cave).expect("c");
        let e = w.spawn_in(c, Pos(0)).expect("spawn");
        let err = w.merge_regions(a, b).expect_err("different frames");
        assert_eq!(err.code().as_str(), "CORE-0004");
        assert_eq!(w.merge_regions(a, c).expect("same frame"), a);
        assert_eq!(w.regions().owner_of(e), Some(a));
        assert_eq!(w.region(a).expect("a").profile(), Profile::Surface);
        assert!(w.region(c).is_err(), "absorbed region is gone");
        w.regions().check_invariants().expect("invariants");
    }

    #[test]
    fn split_is_atomic() {
        let mut w = World::new();
        let a = w.create_region(FrameId(3), Profile::Crowd).expect("a");
        let e1 = w.spawn_in(a, Pos(1)).expect("e1");
        let e2 = w.spawn_in(a, Pos(2)).expect("e2");
        let stray = w.spawn(Pos(3));
        let n = w.regions().len();
        assert_eq!(
            w.split_region(a, &[e1, stray]).err(),
            Some(CoreError::NotOwned {
                entity: stray,
                region: a
            })
        );
        assert_eq!(w.regions().len(), n, "a failed split creates no region");
        assert!(w.region(a).expect("a").owns(e1));
        let b = w.split_region(a, &[e1]).expect("split");
        let rb = w.region(b).expect("b");
        assert_eq!((rb.frame(), rb.profile()), (FrameId(3), Profile::Crowd));
        assert!(rb.owns(e1) && !rb.owns(e2));
        #[cfg(not(feature = "mutate-disjointness"))]
        w.regions().check_invariants().expect("invariants");
    }

    #[test]
    fn view_hands_out_only_owned_and_declared() {
        let mut w = World::new();
        let a = w.create_region(FrameId(0), Profile::Surface).expect("a");
        let b = w.create_region(FrameId(0), Profile::Surface).expect("b");
        let mine = w.spawn_in(a, (Pos(1), Vel(1))).expect("mine");
        let theirs = w.spawn_in(b, (Pos(2), Vel(2))).expect("theirs");
        let access = RegionAccess::new().write::<Pos>().read::<Vel>();
        let results = w
            .with_view(a, &access, |v| {
                let ok_write = v.get_mut::<Pos>(mine).map(|mut p| p.0 += 10).is_ok();
                let ok_read = v.get::<Vel>(mine).map(|x| x.0).ok();
                let not_owned = v.get::<Pos>(theirs).err();
                let undeclared = v.get_mut::<Vel>(mine).err().map(|e| e.code());
                (ok_write, ok_read, not_owned, undeclared)
            })
            .expect("view");
        assert!(results.0);
        assert_eq!(results.1, Some(1));
        assert_eq!(
            results.2,
            Some(CoreError::NotOwned {
                entity: theirs,
                region: a
            })
        );
        assert_eq!(results.3.map(|c| c.as_str()), Some("CORE-0006"));
        assert_eq!(w.get::<Pos>(mine).ok(), Some(&Pos(11)));
        assert_eq!(w.get::<Pos>(theirs).ok(), Some(&Pos(2)));
    }

    #[test]
    fn profile_observation_goes_through_the_region() {
        let mut w = World::new();
        let a = w.create_region(FrameId(0), Profile::Surface).expect("a");
        let cave = ProfileMetrics {
            sky_occlusion: 0.99,
            ..ProfileMetrics::default()
        };
        for _ in 0..crate::ProfileSelector::DEFAULT_DWELL {
            w.observe_profile(a, &cave).expect("observe");
        }
        assert_eq!(w.region(a).expect("a").profile(), Profile::Cave);
    }

    #[test]
    fn an_empty_world_is_small() {
        // Ch.5.2: worlds are cheap. Not a byte budget (bevy's internals may grow), just a
        // sanity bound on the inline size so a regression to something heavy is visible.
        assert!(std::mem::size_of::<World>() < 16 * 1024);
    }
}
