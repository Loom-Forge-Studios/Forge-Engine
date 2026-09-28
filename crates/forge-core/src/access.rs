//! `RegionAccess` — what a region system declares it touches (Ch.5.3).

use std::any::TypeId;

use bevy_ecs::component::{Component, Mutable};

/// A component type, identified for access bookkeeping.
#[derive(Clone, Copy, Debug)]
pub struct ComponentKey {
    id: TypeId,
    name: &'static str,
}

impl ComponentKey {
    /// The key for component `C`.
    #[must_use]
    pub fn of<C: Component>() -> Self {
        Self {
            id: TypeId::of::<C>(),
            name: std::any::type_name::<C>(),
        }
    }

    /// The component's type name (diagnostics only; identity is the `TypeId`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl PartialEq for ComponentKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for ComponentKey {}
impl PartialOrd for ComponentKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ComponentKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

/// The components a region system reads and writes, **within its own region**.
///
/// A system never touches another region's entities at all (cross-region effects go through
/// the [`crate::Outbox`]), so across regions the scheduler proves disjointness from the
/// regions' entity sets; within one region it proves it from these declarations. The
/// declaration is enforced at run time by [`crate::RegionView`]: touching an undeclared
/// component is `CORE-0006`, so the proof never rests on an unchecked promise.
///
/// Kept as two small sorted vectors: a system touches a handful of components, and a sorted
/// merge is the cheapest conflict test at that size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionAccess {
    reads: Vec<ComponentKey>,
    writes: Vec<ComponentKey>,
}

fn insert_sorted(v: &mut Vec<ComponentKey>, k: ComponentKey) {
    if let Err(i) = v.binary_search(&k) {
        v.insert(i, k);
    }
}

impl RegionAccess {
    /// Touches nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Also reads `C`.
    #[must_use]
    pub fn read<C: Component>(mut self) -> Self {
        insert_sorted(&mut self.reads, ComponentKey::of::<C>());
        self
    }

    /// Also writes (and therefore reads) `C`.
    #[must_use]
    pub fn write<C: Component<Mutability = Mutable>>(mut self) -> Self {
        insert_sorted(&mut self.writes, ComponentKey::of::<C>());
        self
    }

    /// May the system read `C`? (A write grants a read.)
    #[must_use]
    pub fn can_read(&self, k: &ComponentKey) -> bool {
        self.reads.binary_search(k).is_ok() || self.writes.binary_search(k).is_ok()
    }

    /// May the system write `C`?
    #[must_use]
    pub fn can_write(&self, k: &ComponentKey) -> bool {
        self.writes.binary_search(k).is_ok()
    }

    /// Declared reads (excluding those also written).
    #[must_use]
    pub fn reads(&self) -> &[ComponentKey] {
        &self.reads
    }

    /// Declared writes.
    #[must_use]
    pub fn writes(&self) -> &[ComponentKey] {
        &self.writes
    }

    /// The first component on which `self` and `other` conflict — one writes what the other
    /// reads or writes — or `None` if they may run concurrently on the same entities.
    #[must_use]
    pub fn conflict_with(&self, other: &RegionAccess) -> Option<ComponentKey> {
        self.writes
            .iter()
            .find(|k| other.can_read(k))
            .or_else(|| other.writes.iter().find(|k| self.can_read(k)))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct A;
    #[derive(Component)]
    struct B;
    #[derive(Component)]
    struct C;

    #[test]
    fn conflicts() {
        let ra = RegionAccess::new().read::<A>();
        let rb = RegionAccess::new().read::<A>().read::<B>();
        let wa = RegionAccess::new().write::<A>();
        let wc = RegionAccess::new().write::<C>().read::<B>();
        assert_eq!(ra.conflict_with(&rb), None, "read/read never conflicts");
        assert_eq!(ra.conflict_with(&wa), Some(ComponentKey::of::<A>()));
        assert_eq!(wa.conflict_with(&ra), Some(ComponentKey::of::<A>()));
        assert_eq!(wa.conflict_with(&wa), Some(ComponentKey::of::<A>()));
        assert_eq!(wa.conflict_with(&wc), None);
        assert_eq!(rb.conflict_with(&wc), None);
        assert!(wa.can_read(&ComponentKey::of::<A>()), "write grants read");
        assert!(!ra.can_write(&ComponentKey::of::<A>()));
    }

    #[test]
    fn declarations_are_sets() {
        let a = RegionAccess::new()
            .read::<A>()
            .read::<A>()
            .write::<B>()
            .write::<B>();
        assert_eq!(a.reads().len(), 1);
        assert_eq!(a.writes().len(), 1);
    }
}
