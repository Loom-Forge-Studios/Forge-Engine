//! [`ExtensionPoint`] and [`Registry`] — ordered, keyed, **replaceable** (Ch.32.2).
//!
//! "Modify any feature" needs more than `add`: a plugin can `replace` a built-in, `remove`
//! it, or `chain` it (wrap it, keeping the original inside — the middleware case most
//! "modify" requests actually want). Every operation records who did it, and two plugins
//! modifying one item incompatibly is an error naming both, never a silent last-wins.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use crate::id::check_key;
use crate::{ItemId, PluginError, PluginId};

/// A named extension point: a kind of thing plugins can add, replace, remove or chain.
/// Each chapter defines its own (Ch.32.2's seed list; [`crate::points`] holds the ones this
/// crate can define without depending upward).
pub trait ExtensionPoint: 'static {
    /// What the registry holds.
    type Item: 'static;
    /// Stable, namespaced, greppable id (`forge.editor.panel`). Never renamed.
    const ID: &'static str;
    /// The name manifests use for it (`EditorPanel("rivers")`).
    const NAME: &'static str;
}

/// Where [`Registry::add`] places a new item. Placement is resolved at insertion, so the
/// final order is deterministic for a given sequence of operations (and the loader applies
/// operations in a fixed order, independent of plugin discovery order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Order {
    /// Before every existing item.
    First,
    /// After every existing item.
    Last,
    /// Immediately before the item with this key.
    Before(String),
    /// Immediately after the item with this key.
    After(String),
}

/// What [`Registry::replace`] returns: the item that was displaced, and whose it was.
#[derive(Debug)]
pub struct Replaced<T> {
    /// The displaced item (a panel manager can offer "restore built-in" with it).
    pub old: T,
    /// Who provided the displaced item: the owner, or the previous replacer.
    pub previous: PluginId,
}

/// What [`Registry::remove`] returns.
#[derive(Debug)]
pub struct Removed<T> {
    /// The removed item.
    pub item: T,
    /// Its owner.
    pub owner: PluginId,
}

/// Who did what to one item: the plugin manager lists this, and conflict errors use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// The plugin that added it.
    pub owner: PluginId,
    /// The plugin that replaced it, if any.
    pub replaced_by: Option<PluginId>,
    /// The plugins that chained it, innermost first.
    pub chained_by: Vec<PluginId>,
}

struct Entry<T> {
    key: Arc<str>,
    prov: Provenance,
    /// `None` only transiently, while a `chain` wrapper owns the item.
    item: Option<T>,
}

/// The items of one extension point: ordered, keyed, replaceable.
pub struct Registry<P: ExtensionPoint> {
    entries: Vec<Entry<P::Item>>,
}

impl<P: ExtensionPoint> Default for Registry<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: ExtensionPoint> Registry<P> {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn pos(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|e| &*e.key == key)
    }

    fn target(&self, target: &ItemId) -> Result<usize, PluginError> {
        if target.point() != P::ID {
            return Err(PluginError::WrongPoint {
                item: target.to_string(),
                expected: P::ID,
            });
        }
        self.pos(target.key())
            .ok_or_else(|| PluginError::UnknownItem(target.to_string()))
    }

    /// Add `item` under `key`, owned by `owner`, placed by `order`.
    pub fn add(
        &mut self,
        owner: PluginId,
        key: &str,
        item: P::Item,
        order: Order,
    ) -> Result<ItemId, PluginError> {
        check_key(key)?;
        if let Some(i) = self.pos(key) {
            return Err(PluginError::DuplicateItem {
                item: format!("{}/{key}", P::ID),
                first: self.entries[i].prov.owner.to_string(),
                second: owner.to_string(),
            });
        }
        let unknown = |k: &str| PluginError::UnknownItem(format!("{}/{k}", P::ID));
        let at = match &order {
            Order::First => 0,
            Order::Last => self.entries.len(),
            Order::Before(k) => self.pos(k).ok_or_else(|| unknown(k))?,
            Order::After(k) => self.pos(k).ok_or_else(|| unknown(k))? + 1,
        };
        let key: Arc<str> = Arc::from(key);
        self.entries.insert(
            at,
            Entry {
                key: Arc::clone(&key),
                prov: Provenance {
                    owner,
                    replaced_by: None,
                    chained_by: Vec::new(),
                },
                item: Some(item),
            },
        );
        Ok(ItemId::from_parts(P::ID, key))
    }

    /// Replace `target` with `item` on behalf of `by`, keeping its position. Refused
    /// (`PLUGIN-0006`, naming both) if another plugin already replaced it. Earlier chains are
    /// dropped with the item they wrapped: a replacement is a new implementation.
    pub fn replace(
        &mut self,
        by: &PluginId,
        target: &ItemId,
        item: P::Item,
    ) -> Result<Replaced<P::Item>, PluginError> {
        let i = self.target(target)?;
        let e = &mut self.entries[i];
        if let Some(prev) = &e.prov.replaced_by
            && prev != by
        {
            return Err(PluginError::Conflict {
                item: target.to_string(),
                first: prev.to_string(),
                first_op: "replaces",
                second: by.to_string(),
                second_op: "replaces",
            });
        }
        let previous = e
            .prov
            .replaced_by
            .clone()
            .unwrap_or_else(|| e.prov.owner.clone());
        let old = e.item.replace(item);
        e.prov.replaced_by = Some(by.clone());
        e.prov.chained_by.clear();
        match old {
            Some(old) => Ok(Replaced { old, previous }),
            None => Err(PluginError::UnknownItem(target.to_string())),
        }
    }

    /// Remove `target` on behalf of `by`. Refused (`PLUGIN-0006`) if another plugin replaced
    /// it: one plugin's replacement is never deleted from under it by another.
    pub fn remove(
        &mut self,
        by: &PluginId,
        target: &ItemId,
    ) -> Result<Removed<P::Item>, PluginError> {
        let i = self.target(target)?;
        if let Some(prev) = &self.entries[i].prov.replaced_by
            && prev != by
        {
            return Err(PluginError::Conflict {
                item: target.to_string(),
                first: prev.to_string(),
                first_op: "replaces",
                second: by.to_string(),
                second_op: "removes",
            });
        }
        let e = self.entries.remove(i);
        e.item
            .map(|item| Removed {
                item,
                owner: e.prov.owner,
            })
            .ok_or_else(|| PluginError::UnknownItem(target.to_string()))
    }

    /// Wrap `target` with `wrap` on behalf of `by`: the wrapper receives the current item
    /// (the built-in, a replacement, or an earlier wrapper) and returns the item that takes
    /// its place. Chains compose; nothing is discarded. A panicking wrapper is caught
    /// (`PLUGIN-0013`); the item it had taken is gone, so the entry is removed rather than
    /// left half-built.
    pub fn chain(
        &mut self,
        by: &PluginId,
        target: &ItemId,
        wrap: impl FnOnce(P::Item) -> P::Item,
    ) -> Result<(), PluginError> {
        let i = self.target(target)?;
        let Some(inner) = self.entries[i].item.take() else {
            return Err(PluginError::UnknownItem(target.to_string()));
        };
        match panic::catch_unwind(AssertUnwindSafe(|| wrap(inner))) {
            Ok(outer) => {
                let e = &mut self.entries[i];
                e.item = Some(outer);
                e.prov.chained_by.push(by.clone());
                Ok(())
            }
            Err(payload) => {
                self.entries.remove(i);
                Err(PluginError::Panicked {
                    plugin: by.to_string(),
                    during: format!("chain of {target}"),
                    message: panic_message(payload.as_ref()),
                })
            }
        }
    }

    /// The item under `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&P::Item> {
        self.pos(key).and_then(|i| self.entries[i].item.as_ref())
    }

    /// Who added, replaced and chained the item under `key`.
    #[must_use]
    pub fn provenance(&self, key: &str) -> Option<&Provenance> {
        self.pos(key).map(|i| &self.entries[i].prov)
    }

    /// The items in order, with their keys.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &P::Item)> {
        self.entries
            .iter()
            .filter_map(|e| e.item.as_ref().map(|it| (&*e.key, it)))
    }

    /// The keys in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| &*e.key)
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Words;
    impl ExtensionPoint for Words {
        type Item = String;
        const ID: &'static str = "test.words";
        const NAME: &'static str = "Word";
    }

    fn pid(s: &str) -> PluginId {
        PluginId::new(s).expect("valid id")
    }

    fn order(r: &Registry<Words>) -> Vec<String> {
        r.iter().map(|(_, v)| v.clone()).collect()
    }

    #[test]
    fn add_replace_remove_chain_and_order() {
        let core = pid("forge.core");
        let mut r = Registry::<Words>::new();
        let a = r
            .add(core.clone(), "a", "A".into(), Order::Last)
            .expect("a");
        let c = r
            .add(core.clone(), "c", "C".into(), Order::Last)
            .expect("c");
        r.add(core.clone(), "b", "B".into(), Order::After("a".into()))
            .expect("b");
        r.add(core.clone(), "z", "Z".into(), Order::First)
            .expect("z");
        r.add(core.clone(), "y", "Y".into(), Order::Before("c".into()))
            .expect("y");
        assert_eq!(order(&r), ["Z", "A", "B", "Y", "C"]);

        let p = pid("com.example.p");
        let rep = r.replace(&p, &a, "A2".into()).expect("replace");
        assert_eq!(
            (rep.old.as_str(), rep.previous.as_str()),
            ("A", "forge.core")
        );
        assert_eq!(order(&r), ["Z", "A2", "B", "Y", "C"], "position kept");

        r.chain(&p, &c, |inner| format!("wrap({inner})"))
            .expect("chain");
        r.chain(&core, &c, |inner| format!("log({inner})"))
            .expect("chain");
        assert_eq!(r.get("c").map(String::as_str), Some("log(wrap(C))"));
        assert_eq!(
            r.provenance("c").map(|p| p.chained_by.len()),
            Some(2),
            "both wrappers recorded"
        );

        let gone = r
            .remove(&p, &ItemId::of::<Words>("z").expect("id"))
            .expect("remove");
        assert_eq!(
            (gone.item.as_str(), gone.owner.as_str()),
            ("Z", "forge.core")
        );
        assert_eq!(order(&r), ["A2", "B", "Y", "log(wrap(C))"]);
    }

    #[test]
    fn conflicts_name_both_plugins() {
        let mut r = Registry::<Words>::new();
        let a = r
            .add(pid("forge.core"), "a", "A".into(), Order::Last)
            .expect("a");
        r.replace(&pid("com.one.x"), &a, "1".into()).expect("first");
        let e = r
            .replace(&pid("com.two.y"), &a, "2".into())
            .expect_err("second replace conflicts");
        assert_eq!(e.code().as_str(), "PLUGIN-0006");
        let msg = e.to_string();
        assert!(
            msg.contains("com.one.x") && msg.contains("com.two.y"),
            "{msg}"
        );
        assert_eq!(r.get("a").map(String::as_str), Some("1"), "no last-wins");
        let e = r
            .remove(&pid("com.two.y"), &a)
            .expect_err("remove conflicts");
        assert_eq!(e.code().as_str(), "PLUGIN-0006");
        // The replacer itself may update or remove its own replacement.
        r.replace(&pid("com.one.x"), &a, "1b".into())
            .expect("re-replace");
        r.remove(&pid("com.one.x"), &a).expect("own removal");
    }

    #[test]
    fn refusals_are_coded() {
        let mut r = Registry::<Words>::new();
        let core = pid("forge.core");
        r.add(core.clone(), "a", "A".into(), Order::Last)
            .expect("a");
        let code = |e: PluginError| e.code().as_str();
        assert_eq!(
            r.add(pid("com.x.y"), "a", "A".into(), Order::Last)
                .map_err(code)
                .err(),
            Some("PLUGIN-0003")
        );
        assert_eq!(
            r.add(core.clone(), "b", "B".into(), Order::After("nope".into()))
                .map_err(code)
                .err(),
            Some("PLUGIN-0004")
        );
        let missing = ItemId::of::<Words>("nope").expect("id");
        assert_eq!(
            r.remove(&core, &missing).map_err(code).err(),
            Some("PLUGIN-0004")
        );
        struct Other;
        impl ExtensionPoint for Other {
            type Item = ();
            const ID: &'static str = "test.other";
            const NAME: &'static str = "Other";
        }
        let wrong = ItemId::of::<Other>("a").expect("id");
        assert_eq!(
            r.remove(&core, &wrong).map_err(code).err(),
            Some("PLUGIN-0005")
        );
    }

    #[test]
    fn a_panicking_wrapper_is_caught_and_its_item_removed() {
        let mut r = Registry::<Words>::new();
        let core = pid("forge.core");
        let a = r
            .add(core.clone(), "a", "A".into(), Order::Last)
            .expect("a");
        r.add(core.clone(), "b", "B".into(), Order::Last)
            .expect("b");
        let e = r
            .chain(&pid("com.bad.p"), &a, |_| panic!("wrapper exploded"))
            .expect_err("caught");
        assert_eq!(e.code().as_str(), "PLUGIN-0013");
        assert!(e.to_string().contains("wrapper exploded"));
        assert_eq!(order(&r), ["B"], "no half-built entry is left behind");
    }
}
