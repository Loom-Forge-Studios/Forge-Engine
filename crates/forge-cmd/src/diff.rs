//! [`Diff`] and [`Change`] — the effect of a command, as data.
//!
//! Every change carries both its `before` and its `after`, so every diff is **invertible by
//! construction**: undo is "apply the inverse", redo is "apply it again", and neither needs
//! per-command code. That is what makes I8's "every command is undoable" a structural fact
//! rather than a promise each command author has to keep. It is also what the split editor
//! applies as an optimistic overlay (O-13) — `dry_run` returns exactly the diff `apply`
//! will apply.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{EntityKey, Value};

/// One elementary, invertible change to the project document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    /// An entity came into existence (no properties, no children yet).
    Created {
        /// Its key.
        entity: EntityKey,
        /// Its name.
        name: String,
        /// Its parent (`None`: a root).
        parent: Option<EntityKey>,
    },
    /// An entity was removed. It had no properties and no children left (a despawn removes
    /// those first, as their own changes), so the inverse restores it exactly.
    Removed {
        /// Its key.
        entity: EntityKey,
        /// Its name at removal.
        name: String,
        /// Its parent at removal.
        parent: Option<EntityKey>,
    },
    /// An entity was renamed.
    Renamed {
        /// The entity.
        entity: EntityKey,
        /// Old name.
        before: String,
        /// New name.
        after: String,
    },
    /// An entity moved in the hierarchy.
    Reparented {
        /// The entity.
        entity: EntityKey,
        /// Old parent.
        before: Option<EntityKey>,
        /// New parent.
        after: Option<EntityKey>,
    },
    /// A property was set (`after: Some`), added (`before: None`) or removed (`after: None`).
    Property {
        /// The entity.
        entity: EntityKey,
        /// Its reflect path (`transform.position`). Shared, not copied: the project interns
        /// each property path once (the key of its property map) and every change, merge
        /// key and history slot on that property holds the same `Arc`, so a drag frame costs
        /// no path allocation after the first.
        path: Arc<str>,
        /// Old value.
        before: Option<Value>,
        /// New value.
        after: Option<Value>,
    },
    /// A project setting was set, added or removed.
    Setting {
        /// The key.
        key: String,
        /// Old value.
        before: Option<Value>,
        /// New value.
        after: Option<Value>,
    },
}

/// What a change is "about", for merging inside a transaction.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum MergeKey {
    Name(EntityKey),
    Property(EntityKey, Arc<str>),
    Setting(String),
}

impl Change {
    /// The change that undoes this one.
    #[must_use]
    pub fn inverse(&self) -> Change {
        match self.clone() {
            Self::Created {
                entity,
                name,
                parent,
            } => Self::Removed {
                entity,
                name,
                parent,
            },
            Self::Removed {
                entity,
                name,
                parent,
            } => Self::Created {
                entity,
                name,
                parent,
            },
            Self::Renamed {
                entity,
                before,
                after,
            } => Self::Renamed {
                entity,
                before: after,
                after: before,
            },
            Self::Reparented {
                entity,
                before,
                after,
            } => Self::Reparented {
                entity,
                before: after,
                after: before,
            },
            Self::Property {
                entity,
                path,
                before,
                after,
            } => Self::Property {
                entity,
                path,
                before: after,
                after: before,
            },
            Self::Setting { key, before, after } => Self::Setting {
                key,
                before: after,
                after: before,
            },
        }
    }

    /// The entity this change is about, if any.
    #[must_use]
    pub fn entity(&self) -> Option<EntityKey> {
        match self {
            Self::Created { entity, .. }
            | Self::Removed { entity, .. }
            | Self::Renamed { entity, .. }
            | Self::Reparented { entity, .. }
            | Self::Property { entity, .. } => Some(*entity),
            Self::Setting { .. } => None,
        }
    }

    /// Value-like changes merge inside a transaction (a drag's sixty `SetProperty`s become
    /// one change). Structural changes never merge: moving a reparent earlier could name a
    /// parent created later in the same transaction.
    pub(crate) fn merge_key(&self) -> Option<MergeKey> {
        match self {
            Self::Renamed { entity, .. } => Some(MergeKey::Name(*entity)),
            Self::Property { entity, path, .. } => Some(MergeKey::Property(*entity, path.clone())),
            Self::Setting { key, .. } => Some(MergeKey::Setting(key.clone())),
            Self::Created { .. } | Self::Removed { .. } | Self::Reparented { .. } => None,
        }
    }

    /// Fold `later` (same merge key, applied after `self`) into `self` in place: keep the
    /// earliest `before`, take the latest `after` (only the value is cloned, never the path).
    /// Returns `false` when the result is a no-op (a drag that ended where it started), so
    /// the transaction drops the change entirely.
    pub(crate) fn merge_in_place(&mut self, later: &Change) -> bool {
        match (self, later) {
            (
                Self::Renamed { before, after, .. },
                Self::Renamed {
                    after: la,
                    before: lb,
                    ..
                },
            ) => {
                if cfg!(feature = "mutate-undo") {
                    before.clone_from(lb);
                }
                after.clone_from(la);
                before != after
            }
            (
                Self::Property { before, after, .. },
                Self::Property {
                    after: la,
                    before: lb,
                    ..
                },
            )
            | (
                Self::Setting { before, after, .. },
                Self::Setting {
                    after: la,
                    before: lb,
                    ..
                },
            ) => {
                if cfg!(feature = "mutate-undo") {
                    before.clone_from(lb);
                }
                after.clone_from(la);
                before != after
            }
            // Different kinds never share a merge key; callers never reach this.
            (me, _) => {
                *me = later.clone();
                true
            }
        }
    }
}

/// The effect of a command, an undo, or a redo: an ordered list of changes.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff {
    /// The changes, in the order they apply.
    pub changes: Vec<Change>,
}

impl Diff {
    /// No changes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Number of changes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// The diff that undoes this one: every change inverted, in reverse order.
    #[must_use]
    pub fn inverse(&self) -> Diff {
        Diff {
            changes: self.changes.iter().rev().map(Change::inverse).collect(),
        }
    }
}

/// A transaction's accumulated changes, merged as they arrive: `O(1)` per change, and a
/// drag of any length is one change per property it touched.
#[derive(Clone, Debug, Default)]
pub(crate) struct Accumulator {
    slots: Vec<Option<Change>>,
    index: HashMap<MergeKey, usize>,
    live: usize,
}

impl Accumulator {
    #[cfg(test)]
    pub(crate) fn push(&mut self, change: Change) {
        self.push_ref(&change);
    }

    /// Accumulate `change`. Merging into an existing slot updates it in place and clones
    /// only the new value; the change itself is cloned only when it opens a new slot.
    pub(crate) fn push_ref(&mut self, change: &Change) {
        let Some(key) = change.merge_key() else {
            self.slots.push(Some(change.clone()));
            self.live += 1;
            return;
        };
        if let Some(&i) = self.index.get(&key)
            && let Some(slot) = self.slots.get_mut(i)
        {
            match slot {
                Some(old) => {
                    if !old.merge_in_place(change) {
                        // A no-op: drop the change but keep its slot, so a drag that
                        // crosses its start value again reuses it instead of growing.
                        *slot = None;
                        self.live -= 1;
                    }
                }
                None => {
                    *slot = Some(change.clone());
                    self.live += 1;
                }
            }
            return;
        }
        self.index.insert(key, self.slots.len());
        self.slots.push(Some(change.clone()));
        self.live += 1;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Slots held (live changes plus no-op slots kept for reuse): the memory it holds.
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The net changes, in application order.
    pub(crate) fn changes(&self) -> impl Iterator<Item = &Change> {
        self.slots.iter().flatten()
    }

    #[cfg(test)]
    pub(crate) fn into_changes(self) -> Vec<Change> {
        self.slots.into_iter().flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(before: Option<i64>, after: Option<i64>) -> Change {
        Change::Property {
            entity: EntityKey(1),
            path: "x".into(),
            before: before.map(Value::Int),
            after: after.map(Value::Int),
        }
    }

    #[test]
    fn inverse_of_inverse_is_identity() {
        let d = Diff {
            changes: vec![
                Change::Created {
                    entity: EntityKey(3),
                    name: "a".into(),
                    parent: None,
                },
                prop(None, Some(1)),
                Change::Reparented {
                    entity: EntityKey(3),
                    before: None,
                    after: Some(EntityKey(1)),
                },
            ],
        };
        assert_eq!(d.inverse().inverse(), d);
        assert!(matches!(d.inverse().changes[0], Change::Reparented { .. }));
    }

    #[test]
    #[cfg(not(feature = "mutate-undo"))]
    fn a_drag_merges_to_one_change_with_the_first_before() {
        let mut acc = Accumulator::default();
        acc.push(prop(Some(0), Some(1)));
        acc.push(prop(Some(1), Some(2)));
        acc.push(prop(Some(2), Some(3)));
        let out = acc.into_changes();
        assert_eq!(out, vec![prop(Some(0), Some(3))]);
    }

    #[test]
    fn a_drag_back_to_the_start_is_no_change() {
        let mut acc = Accumulator::default();
        acc.push(prop(Some(0), Some(5)));
        acc.push(prop(Some(5), Some(0)));
        assert!(acc.is_empty());
        acc.push(prop(Some(0), Some(7)));
        assert_eq!(
            acc.slot_count(),
            1,
            "the no-op slot is reused, not appended"
        );
        assert_eq!(acc.into_changes(), vec![prop(Some(0), Some(7))]);
    }

    #[test]
    fn a_drag_oscillating_through_its_start_holds_one_slot() {
        let mut acc = Accumulator::default();
        for i in 0..10_000_i64 {
            let (b, a) = if i % 2 == 0 { (0, 1) } else { (1, 0) };
            acc.push(prop(Some(b), Some(a)));
        }
        assert!(acc.is_empty());
        assert_eq!(acc.slot_count(), 1);
    }

    #[test]
    fn structural_changes_never_merge() {
        let mut acc = Accumulator::default();
        let r = |b: Option<u64>, a: Option<u64>| Change::Reparented {
            entity: EntityKey(1),
            before: b.map(EntityKey),
            after: a.map(EntityKey),
        };
        acc.push(r(None, Some(2)));
        acc.push(r(Some(2), Some(3)));
        assert_eq!(acc.changes().count(), 2);
    }
}
