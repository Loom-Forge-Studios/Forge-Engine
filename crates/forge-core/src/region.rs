//! Regions (Ch.5.3): disjoint sets of entities, each owned by exactly one region.
//!
//! The ownership table here is the single source of truth the scheduler's disjointness proof
//! reads. Every mutation keeps two structures in step — each region's sorted entity set and
//! the dense entity → owner table ([`OwnerTable`], `O(1)` by entity slot index) — and [`Regions::check_invariants`] re-derives the property from
//! scratch, which is what `tests/test_region_disjointness.rs` fuzzes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bevy_ecs::entity::Entity;

use forge_frames::FrameId;

use crate::{CoreError, EntityId, Profile, ProfileMetrics, ProfileSelector};

/// A region's identity. Allocated monotonically and never reused within a world, so a stale
/// id is `CORE-0002`, never a different region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(pub u32);

/// A region owns a disjoint set of entities and may mutate only those. Cross-region effects
/// are messages, applied at a barrier (Ch.5.3).
#[derive(Clone, Debug)]
pub struct Region {
    id: RegionId,
    frame: FrameId,
    selector: ProfileSelector,
    /// Sorted: iteration order is the entity order, identical on every platform.
    entities: BTreeSet<EntityId>,
}

impl Region {
    /// The region's id.
    #[must_use]
    pub fn id(&self) -> RegionId {
        self.id
    }

    /// The frame every entity of this region is expressed in.
    #[must_use]
    pub fn frame(&self) -> FrameId {
        self.frame
    }

    /// The profile in force (Ch.1.4). Changes cost, never outcome (I5).
    #[must_use]
    pub fn profile(&self) -> Profile {
        self.selector.current()
    }

    /// The region's profile selector (measurement + hysteresis).
    #[must_use]
    pub fn selector(&self) -> &ProfileSelector {
        &self.selector
    }

    /// The entities this region owns, in ascending order.
    pub fn entities(&self) -> impl ExactSizeIterator<Item = EntityId> + '_ {
        self.entities.iter().copied()
    }

    /// Number of owned entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Owns nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Does this region own `e`?
    #[must_use]
    pub fn owns(&self, e: EntityId) -> bool {
        self.entities.contains(&e)
    }
}

/// Which region owns each entity, **dense by entity slot index**: an ownership lookup is one
/// bounds-checked index plus one compare, `O(1)`, with no hashing and no tree walk. This is
/// the lookup [`crate::RegionView`] makes on every component access (WP-04 replaced a
/// per-region `BTreeSet` search, `O(log n)`, there).
///
/// Each slot stores the full [`Entity`] (index + generation), so a stale handle to a reused
/// slot never matches: owning `3v0` says nothing about `3v1`. Memory is one slot per entity
/// index up to the highest index ever owned (~1.6 MB at 100k entities); `bevy_ecs` reuses
/// freed slot indices, so the table tracks the live entity count, not the spawn history.
#[derive(Clone, Debug, Default)]
pub struct OwnerTable {
    slots: Vec<Option<(Entity, RegionId)>>,
    len: usize,
}

impl OwnerTable {
    /// The region owning `e`, if any. `O(1)`.
    #[inline]
    #[must_use]
    pub fn get(&self, e: EntityId) -> Option<RegionId> {
        match self.slots.get(e.index() as usize) {
            Some(&Some((ent, r))) if ent == e.entity() => Some(r),
            _ => None,
        }
    }

    /// Does `region` own `e`? `O(1)`.
    #[inline]
    #[must_use]
    pub fn is_owned_by(&self, e: EntityId, region: RegionId) -> bool {
        self.get(e) == Some(region)
    }

    /// Number of owned entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Nothing is owned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Set `e`'s owner; returns the previous owner **of this exact entity**. A slot held by
    /// an older generation of the same index is overwritten (that entity no longer exists:
    /// `World::despawn` releases before `bevy_ecs` frees the slot).
    fn insert(&mut self, e: EntityId, r: RegionId) -> Option<RegionId> {
        let i = e.index() as usize;
        if i >= self.slots.len() {
            self.slots.resize(i + 1, None);
        }
        let prev = self
            .slots
            .get_mut(i)
            .and_then(|s| s.replace((e.entity(), r)));
        match prev {
            Some((ent, old)) if ent == e.entity() => Some(old),
            Some(_) => None,
            None => {
                self.len += 1;
                None
            }
        }
    }

    /// Clear `e`'s owner; returns it.
    fn remove(&mut self, e: EntityId) -> Option<RegionId> {
        let slot = self.slots.get_mut(e.index() as usize)?;
        match *slot {
            Some((ent, r)) if ent == e.entity() => {
                *slot = None;
                self.len -= 1;
                Some(r)
            }
            _ => None,
        }
    }

    /// Every `(entity, owner)`, by slot index. For the full re-derivation only.
    fn iter(&self) -> impl Iterator<Item = (EntityId, RegionId)> + '_ {
        self.slots
            .iter()
            .filter_map(|s| s.map(|(e, r)| (EntityId::from_entity(e), r)))
    }
}

/// The ownership table: every region, and which region owns each entity.
#[derive(Clone, Debug, Default)]
pub struct Regions {
    /// Ordered by id: the scheduler iterates regions in this order.
    regions: BTreeMap<RegionId, Region>,
    /// Dense entity → owner lookup, `O(1)` (see [`OwnerTable`]).
    owner: OwnerTable,
    next: u32,
    /// Every ownership change since the last proof: `(entity, owner before the change)`.
    /// Lets [`Regions::prove_disjoint`] re-check only what moved (see its docs).
    journal: Vec<(EntityId, Option<RegionId>)>,
    /// The journal outgrew the table; the next proof re-derives everything.
    full_check: bool,
}

impl Regions {
    /// Create an empty region in `frame`, starting in `profile`.
    pub fn create(&mut self, frame: FrameId, profile: Profile) -> Result<RegionId, CoreError> {
        let id = RegionId(self.next);
        self.next = self
            .next
            .checked_add(1)
            .ok_or(CoreError::RegionsExhausted)?;
        self.regions.insert(
            id,
            Region {
                id,
                frame,
                selector: ProfileSelector::new(profile),
                entities: BTreeSet::new(),
            },
        );
        Ok(id)
    }

    /// The region with this id.
    pub fn get(&self, id: RegionId) -> Result<&Region, CoreError> {
        self.regions.get(&id).ok_or(CoreError::UnknownRegion(id))
    }

    /// Every region, in id order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Region> + '_ {
        self.regions.values()
    }

    /// Every region id, in order.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = RegionId> + '_ {
        self.regions.keys().copied()
    }

    /// Number of regions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// No regions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// The region owning `e`, if any.
    #[must_use]
    pub fn owner_of(&self, e: EntityId) -> Option<RegionId> {
        self.owner.get(e)
    }

    /// The dense entity → owner table ([`crate::RegionView`] checks every access against it).
    #[must_use]
    pub fn owners(&self) -> &OwnerTable {
        &self.owner
    }

    /// Give an unowned entity to `region`. Assigning to its current owner is a no-op;
    /// assigning an entity another region owns is `CORE-0003` — use [`Regions::transfer`].
    /// (Entity existence is the caller's check; [`crate::World`] makes it.)
    pub fn assign(&mut self, e: EntityId, region: RegionId) -> Result<(), CoreError> {
        match self.owner.get(e) {
            Some(r) if r == region => return Ok(()),
            Some(owner) => return Err(CoreError::AlreadyOwned { entity: e, owner }),
            None => {}
        }
        let r = self
            .regions
            .get_mut(&region)
            .ok_or(CoreError::UnknownRegion(region))?;
        r.entities.insert(e);
        self.owner.insert(e, region);
        self.note(e, None);
        Ok(())
    }

    /// Move `e` to `to`, whoever owns it now (or nobody).
    pub fn transfer(&mut self, e: EntityId, to: RegionId) -> Result<(), CoreError> {
        if !self.regions.contains_key(&to) {
            return Err(CoreError::UnknownRegion(to));
        }
        self.release(e);
        self.assign(e, to)
    }

    /// Make `e` unowned. Returns its former owner.
    pub fn release(&mut self, e: EntityId) -> Option<RegionId> {
        let from = self.owner.remove(e)?;
        self.note(e, Some(from));
        if let Some(r) = self.regions.get_mut(&from) {
            r.entities.remove(&e);
        }
        Some(from)
    }

    /// Absorb region `from` into region `into` (Ch.5.3 "merge on proximity"). Both must share
    /// a frame (`CORE-0004`). `into` keeps its id and profile; `from` ceases to exist.
    /// Merging a region into itself is a no-op.
    pub fn merge(&mut self, into: RegionId, from: RegionId) -> Result<RegionId, CoreError> {
        let into_frame = self.get(into)?.frame;
        let from_frame = self.get(from)?.frame;
        if into == from {
            return Ok(into);
        }
        if into_frame != from_frame {
            return Err(CoreError::FrameMismatch {
                into,
                from,
                into_frame,
                from_frame,
            });
        }
        let absorbed = self
            .regions
            .remove(&from)
            .ok_or(CoreError::UnknownRegion(from))?;
        let target = self
            .regions
            .get_mut(&into)
            .ok_or(CoreError::UnknownRegion(into))?;
        for e in &absorbed.entities {
            self.owner.insert(*e, into);
            self.journal.push((*e, Some(from)));
        }
        target.entities.extend(absorbed.entities);
        self.trim_journal();
        Ok(into)
    }

    /// Split `entities` off `region` into a new region with the same frame and profile
    /// (Ch.5.3 "split on load"). Atomic: every entity must be owned by `region`
    /// (`CORE-0005`), or nothing changes.
    pub fn split(
        &mut self,
        region: RegionId,
        entities: &[EntityId],
    ) -> Result<RegionId, CoreError> {
        let src = self.get(region)?;
        if let Some(&e) = entities.iter().find(|e| !src.entities.contains(e)) {
            return Err(CoreError::NotOwned { entity: e, region });
        }
        let (frame, profile) = (src.frame, src.profile());
        let new = self.create(frame, profile)?;
        let moved: BTreeSet<EntityId> = entities.iter().copied().collect();
        #[cfg(not(feature = "mutate-disjointness"))]
        if let Some(src) = self.regions.get_mut(&region) {
            for e in &moved {
                src.entities.remove(e);
            }
        }
        // (With `mutate-disjointness` the moved entities stay in the source region too — the
        // deliberate bookkeeping bug the disjointness property test must catch.)
        for e in &moved {
            self.owner.insert(*e, new);
            self.journal.push((*e, Some(region)));
        }
        if let Some(dst) = self.regions.get_mut(&new) {
            dst.entities = moved;
        }
        self.trim_journal();
        Ok(new)
    }

    /// Delete a region; its entities become unowned. Returns them.
    pub fn remove(&mut self, id: RegionId) -> Result<Vec<EntityId>, CoreError> {
        let r = self
            .regions
            .remove(&id)
            .ok_or(CoreError::UnknownRegion(id))?;
        for e in &r.entities {
            self.owner.remove(*e);
            self.journal.push((*e, Some(id)));
        }
        self.trim_journal();
        Ok(r.entities.into_iter().collect())
    }

    /// Feed a measurement to a region's profile selector; returns the profile in force.
    pub fn observe_profile(
        &mut self,
        id: RegionId,
        m: &ProfileMetrics,
    ) -> Result<Profile, CoreError> {
        let r = self
            .regions
            .get_mut(&id)
            .ok_or(CoreError::UnknownRegion(id))?;
        Ok(r.selector.observe(m))
    }

    fn note(&mut self, e: EntityId, old: Option<RegionId>) {
        self.journal.push((e, old));
        self.trim_journal();
    }

    /// A journal longer than the table costs more to replay than a full re-derivation.
    fn trim_journal(&mut self) {
        if self.journal.len() > self.owner.len().max(64) {
            self.journal.clear();
            self.full_check = true;
        }
    }

    /// Prove the ownership property **incrementally** — the scheduler's per-tick proof.
    ///
    /// After a successful proof every entity in every region's set is owned (in the owner
    /// map) by exactly that region, so the sets are disjoint. Every change to membership since
    /// then went through this type's methods, each of which journals the entity and its
    /// previous owner. Replaying the journal — the entity is in the set of the region that now
    /// owns it, and **not** in the set of the region that owned it before — re-establishes the
    /// property at a cost proportional to what moved, not to the world: `O(moved · log n)`
    /// instead of the `O(entities)` hash-everything of [`Regions::check_invariants`] (2.2 ms
    /// per tick at 100k entities, ADR 0005).
    ///
    /// On failure the journal is kept, so the next attempt fails too.
    pub fn prove_disjoint(&mut self) -> Result<(), CoreError> {
        if self.full_check {
            self.check_invariants()?;
            self.full_check = false;
            return Ok(());
        }
        for &(e, old) in &self.journal {
            let now = self.owner.get(e);
            if let Some(r) = now
                && !self
                    .regions
                    .get(&r)
                    .is_some_and(|reg| reg.entities.contains(&e))
            {
                return Err(CoreError::NotOwned {
                    entity: e,
                    region: r,
                });
            }
            if let Some(o) = old
                && Some(o) != now
                && self
                    .regions
                    .get(&o)
                    .is_some_and(|reg| reg.entities.contains(&e))
            {
                return Err(match now {
                    Some(r) => CoreError::Overlap {
                        entity: e,
                        a: o,
                        b: r,
                    },
                    None => CoreError::NotOwned {
                        entity: e,
                        region: o,
                    },
                });
            }
        }
        self.journal.clear();
        Ok(())
    }

    /// Re-derive the ownership property from scratch: every entity is in at most one region's
    /// set, and the owner map agrees with the sets exactly. `O(entities)`. Returns the first
    /// violation found.
    pub fn check_invariants(&self) -> Result<(), CoreError> {
        let mut seen: HashMap<EntityId, RegionId> = HashMap::with_capacity(self.owner.len());
        for r in self.regions.values() {
            for &e in &r.entities {
                if let Some(prev) = seen.insert(e, r.id) {
                    return Err(CoreError::Overlap {
                        entity: e,
                        a: prev,
                        b: r.id,
                    });
                }
                if self.owner.get(e) != Some(r.id) {
                    return Err(CoreError::NotOwned {
                        entity: e,
                        region: r.id,
                    });
                }
            }
        }
        if seen.len() != self.owner.len() {
            // The owner map names an entity no region set holds.
            if let Some((e, r)) = self.owner.iter().find(|(e, _)| !seen.contains_key(e)) {
                return Err(CoreError::NotOwned {
                    entity: e,
                    region: r,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(i: u32) -> EntityId {
        EntityId::from_entity(
            bevy_ecs::entity::Entity::from_raw_u32(i)
                .unwrap_or(bevy_ecs::entity::Entity::PLACEHOLDER),
        )
    }

    #[test]
    fn incremental_proof_agrees_with_full_derivation() {
        let mut t = Regions::default();
        let a = t.create(FrameId(0), Profile::Surface).expect("a");
        let b = t.create(FrameId(0), Profile::Surface).expect("b");
        for i in 1..20 {
            t.assign(ent(i), if i % 2 == 0 { a } else { b })
                .expect("assign");
        }
        t.prove_disjoint().expect("proof");
        assert!(
            t.journal.is_empty(),
            "a successful proof consumes the journal"
        );
        t.transfer(ent(2), b).expect("transfer");
        let c = t.split(b, &[ent(1), ent(3)]).expect("split");
        t.merge(a, c).expect("merge");
        t.release(ent(5));
        assert_eq!(t.journal.len(), 7);
        t.prove_disjoint().expect("proof");
        t.check_invariants().expect("full");
    }

    #[test]
    fn owner_table_matches_exact_generation_only() {
        let mut w = bevy_ecs::world::World::new();
        let old = EntityId::from_entity(w.spawn_empty().id());
        let mut t = OwnerTable::default();
        let r = RegionId(7);
        assert_eq!(t.insert(old, r), None);
        assert_eq!(t.len(), 1);
        assert!(t.is_owned_by(old, r));
        assert!(!t.is_owned_by(old, RegionId(8)));
        // Free the slot; the same index comes back with the next generation (the high 32
        // bits of the entity's bit form).
        assert_eq!(t.remove(old), Some(r));
        let new = EntityId::from_bits(old.to_bits() + (1 << 32)).expect("next generation");
        assert_eq!(new.index(), old.index());
        assert_ne!(new, old);
        assert_eq!(t.insert(new, r), None);
        assert!(t.is_owned_by(new, r));
        assert_eq!(
            t.get(old),
            None,
            "a stale handle never matches a reused slot"
        );
        assert_eq!(t.remove(old), None);
        assert_eq!(t.len(), 1);
        // An index past the end is simply unowned.
        assert_eq!(t.get(ent(10_000)), None);
    }

    #[test]
    fn a_long_journal_falls_back_to_one_full_derivation() {
        let mut t = Regions::default();
        let a = t.create(FrameId(0), Profile::Surface).expect("a");
        let b = t.create(FrameId(0), Profile::Surface).expect("b");
        t.assign(ent(1), a).expect("assign");
        for i in 0..100 {
            t.transfer(ent(1), if i % 2 == 0 { b } else { a })
                .expect("transfer");
        }
        assert!(t.full_check && t.journal.len() <= 64, "journal is bounded");
        t.prove_disjoint().expect("proof");
        assert!(!t.full_check);
    }
}
