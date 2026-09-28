//! [`Extensions`] — the host holding one [`Registry`] per defined extension point.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};

use crate::{ExtensionPoint, PluginError, Provenance, Registry};

/// A defined extension point, as the plugin manager and the conformance guard list it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointInfo {
    /// Its id (`forge.editor.panel`).
    pub id: &'static str,
    /// Its manifest name (`EditorPanel`).
    pub name: &'static str,
}

struct Slot {
    ty: TypeId,
    name: &'static str,
    reg: Box<dyn Any>,
    inventory: fn(&dyn Any) -> Vec<(String, Provenance)>,
}

fn inventory_of<P: ExtensionPoint>(reg: &dyn Any) -> Vec<(String, Provenance)> {
    reg.downcast_ref::<Registry<P>>()
        .map(|r| {
            r.keys()
                .filter_map(|k| r.provenance(k).map(|p| (k.to_string(), p.clone())))
                .collect()
        })
        .unwrap_or_default()
}

/// Every extension point this engine instance defines, each with its registry. First-party
/// subsystems and third-party plugins reach it through the same methods (I16).
#[derive(Default)]
pub struct Extensions {
    slots: BTreeMap<&'static str, Slot>,
    by_type: HashMap<TypeId, &'static str>,
    by_name: BTreeMap<&'static str, &'static str>,
}

impl Extensions {
    /// No points defined.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Define point `P` (idempotent). Refused (`PLUGIN-0012`) if its id or manifest name is
    /// already taken by a different item type.
    pub fn define<P: ExtensionPoint>(&mut self) -> Result<(), PluginError> {
        let ty = TypeId::of::<Registry<P>>();
        if let Some(slot) = self.slots.get(P::ID) {
            return if slot.ty == ty {
                Ok(())
            } else {
                Err(PluginError::PointIdTaken(P::ID))
            };
        }
        if self.by_name.contains_key(P::NAME) || self.by_type.contains_key(&ty) {
            return Err(PluginError::PointIdTaken(P::ID));
        }
        self.slots.insert(
            P::ID,
            Slot {
                ty,
                name: P::NAME,
                reg: Box::new(Registry::<P>::new()),
                inventory: inventory_of::<P>,
            },
        );
        self.by_type.insert(ty, P::ID);
        self.by_name.insert(P::NAME, P::ID);
        Ok(())
    }

    /// The registry of `P`, if defined.
    #[must_use]
    pub fn registry<P: ExtensionPoint>(&self) -> Option<&Registry<P>> {
        self.slots
            .get(P::ID)
            .and_then(|s| s.reg.downcast_ref::<Registry<P>>())
    }

    /// The registry of `P`, mutably, if defined.
    pub fn registry_mut<P: ExtensionPoint>(&mut self) -> Option<&mut Registry<P>> {
        self.slots
            .get_mut(P::ID)
            .and_then(|s| s.reg.downcast_mut::<Registry<P>>())
    }

    /// The registry of `P`, or `PLUGIN-0010` naming `who` if `P` is not defined.
    pub(crate) fn require_mut<P: ExtensionPoint>(
        &mut self,
        who: &str,
    ) -> Result<&mut Registry<P>, PluginError> {
        match self.slots.get(P::ID).map(|s| s.ty) {
            Some(ty) if ty != TypeId::of::<Registry<P>>() => Err(PluginError::PointIdTaken(P::ID)),
            Some(_) => self
                .registry_mut::<P>()
                .ok_or(PluginError::PointIdTaken(P::ID)),
            None => Err(PluginError::UnknownPoint {
                plugin: who.to_string(),
                point: P::ID.to_string(),
            }),
        }
    }

    /// The point id a manifest name refers to.
    #[must_use]
    pub fn id_for_name(&self, name: &str) -> Option<&'static str> {
        self.by_name.get(name).copied()
    }

    /// Every defined point, in id order.
    pub fn points(&self) -> impl Iterator<Item = PointInfo> + '_ {
        self.slots
            .iter()
            .map(|(id, s)| PointInfo { id, name: s.name })
    }

    /// Every item of every point with its provenance (point, key, owner / replacer / chain),
    /// in point-id then registry order: what each plugin contributes, for the plugin manager
    /// and the I16 guard.
    pub fn inventory(&self) -> Vec<(PointInfo, String, Provenance)> {
        let mut out = Vec::new();
        for (id, s) in &self.slots {
            let info = PointInfo { id, name: s.name };
            for (k, p) in (s.inventory)(&*s.reg) {
                out.push((info, k, p));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    impl ExtensionPoint for A {
        type Item = u32;
        const ID: &'static str = "test.a";
        const NAME: &'static str = "A";
    }
    struct Impostor;
    impl ExtensionPoint for Impostor {
        type Item = String;
        const ID: &'static str = "test.a";
        const NAME: &'static str = "Impostor";
    }
    struct SameName;
    impl ExtensionPoint for SameName {
        type Item = String;
        const ID: &'static str = "test.b";
        const NAME: &'static str = "A";
    }

    #[test]
    fn a_point_id_or_name_belongs_to_one_item_type() {
        let mut x = Extensions::new();
        x.define::<A>().expect("define");
        x.define::<A>().expect("idempotent");
        assert_eq!(
            x.define::<Impostor>().map_err(|e| e.code().as_str()),
            Err("PLUGIN-0012")
        );
        assert_eq!(
            x.define::<SameName>().map_err(|e| e.code().as_str()),
            Err("PLUGIN-0012")
        );
        assert!(x.registry::<Impostor>().is_none());
        assert_eq!(x.id_for_name("A"), Some("test.a"));
        assert_eq!(x.points().count(), 1);
        assert!(x.registry::<A>().is_some_and(Registry::is_empty));
    }
}
