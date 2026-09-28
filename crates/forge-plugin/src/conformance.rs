//! `test_extension_point_replaceable` as a reusable check (Ch.32.2): every extension point
//! supports `add`, `replace`, `remove` and `chain` — exercised, observed through the items
//! themselves, not through the registry's own bookkeeping.
//!
//! Generic over [`RegistryOps`] so the guard's positive controls can feed it deliberately
//! broken registries, and over a [`Kit`] per point so each point's items are observed by
//! *using* them (a command is dry-run on a real bus, a store backend is opened).

use std::fmt;

use crate::{ExtensionPoint, ItemId, Order, PluginError, PluginId, Registry, Replaced};

/// The operations the check drives. [`Registry`] implements it; positive controls wrap it
/// with faults.
pub trait RegistryOps<P: ExtensionPoint> {
    /// See [`Registry::add`].
    fn add(
        &mut self,
        owner: PluginId,
        key: &str,
        item: P::Item,
        order: Order,
    ) -> Result<ItemId, PluginError>;
    /// See [`Registry::replace`].
    fn replace(
        &mut self,
        by: &PluginId,
        target: &ItemId,
        item: P::Item,
    ) -> Result<Replaced<P::Item>, PluginError>;
    /// See [`Registry::remove`] (the removed item is returned).
    fn remove(&mut self, by: &PluginId, target: &ItemId) -> Result<P::Item, PluginError>;
    /// See [`Registry::chain`].
    fn chain(
        &mut self,
        by: &PluginId,
        target: &ItemId,
        wrap: Box<dyn FnOnce(P::Item) -> P::Item>,
    ) -> Result<(), PluginError>;
    /// The items, in order.
    fn items(&self) -> Vec<&P::Item>;
}

impl<P: ExtensionPoint> RegistryOps<P> for Registry<P> {
    fn add(
        &mut self,
        owner: PluginId,
        key: &str,
        item: P::Item,
        order: Order,
    ) -> Result<ItemId, PluginError> {
        Registry::add(self, owner, key, item, order)
    }
    fn replace(
        &mut self,
        by: &PluginId,
        target: &ItemId,
        item: P::Item,
    ) -> Result<Replaced<P::Item>, PluginError> {
        Registry::replace(self, by, target, item)
    }
    fn remove(&mut self, by: &PluginId, target: &ItemId) -> Result<P::Item, PluginError> {
        Registry::remove(self, by, target).map(|r| r.item)
    }
    fn chain(
        &mut self,
        by: &PluginId,
        target: &ItemId,
        wrap: Box<dyn FnOnce(P::Item) -> P::Item>,
    ) -> Result<(), PluginError> {
        Registry::chain(self, by, target, wrap)
    }
    fn items(&self) -> Vec<&P::Item> {
        self.iter().map(|(_, it)| it).collect()
    }
}

/// How the check makes and observes one point's items.
pub struct Kit<P: ExtensionPoint> {
    /// An item whose behaviour is tagged `tag`.
    pub make: fn(&str) -> P::Item,
    /// Observe an item's behaviour by using it; `make(t)` must observe as `t`.
    pub probe: fn(&P::Item) -> String,
    /// A middleware wrapper; `probe(wrap(x))` must be `format!("wrap({})", probe(x))`.
    pub wrap: fn(P::Item) -> P::Item,
}

/// Which clause failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// The kit is broken (`make`/`probe` disagree): the check would be vacuous.
    VacuousKit,
    /// `add` refused a valid item, or `Order` was not honoured.
    Add,
    /// `replace` did not put the new item in the old one's place, or lost the old item.
    Replace,
    /// `remove` did not remove it, or did not return it.
    Remove,
    /// `chain` did not wrap the item (discarded it, or ignored the wrapper).
    Chain,
    /// A second plugin's replace of an already-replaced item was not a conflict naming both.
    Conflict,
    /// An operation on a missing item was not `PLUGIN-0004`.
    Unknown,
}

/// One failed clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The point.
    pub point: &'static str,
    /// The clause.
    pub rule: Rule,
    /// Details.
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}: {}", self.point, self.rule, self.detail)
    }
}

/// Run every clause on a fresh registry from `make`.
pub fn check_replaceable<P: ExtensionPoint, R: RegistryOps<P>>(
    make: impl Fn() -> R,
    kit: &Kit<P>,
) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut v = |rule: Rule, detail: String| {
        out.push(Violation {
            point: P::ID,
            rule,
            detail,
        });
    };
    let probe = |r: &R| -> Vec<String> { r.items().into_iter().map(kit.probe).collect() };

    // The kit itself must be able to tell items apart, or every clause passes vacuously.
    let a = (kit.make)("A");
    if (kit.probe)(&a) != "A" || (kit.probe)(&(kit.make)("B")) == "A" {
        v(Rule::VacuousKit, "probe(make(t)) must be t".into());
        return out;
    }
    if (kit.probe)(&(kit.wrap)(a)) != "wrap(A)" {
        v(
            Rule::VacuousKit,
            "probe(wrap(x)) must be wrap(probe(x))".into(),
        );
        return out;
    }

    let (Ok(core), Ok(one), Ok(two)) = (
        PluginId::new("forge.core"),
        PluginId::new("com.one.plugin"),
        PluginId::new("com.two.plugin"),
    ) else {
        v(
            Rule::VacuousKit,
            "the check's own plugin ids are invalid".into(),
        );
        return out;
    };
    let mut r = make();
    let ids: Result<Vec<ItemId>, PluginError> = [
        ("a", "A", Order::Last),
        ("c", "C", Order::Last),
        ("b", "B", Order::After("a".into())),
        ("z", "Z", Order::First),
    ]
    .into_iter()
    .map(|(k, t, o)| r.add(core.clone(), k, (kit.make)(t), o))
    .collect();
    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => {
            v(Rule::Add, format!("add refused: {e}"));
            return out;
        }
    };
    let (a, c, b, z) = (&ids[0], &ids[1], &ids[2], &ids[3]);
    if probe(&r) != ["Z", "A", "B", "C"] {
        v(Rule::Add, format!("order not honoured: {:?}", probe(&r)));
    }

    // replace: same place, old item handed back.
    match r.replace(&one, b, (kit.make)("B2")) {
        Ok(rep) if (kit.probe)(&rep.old) == "B" && rep.previous == core => {}
        Ok(rep) => v(
            Rule::Replace,
            format!(
                "returned {:?} from {}, expected B from forge.core",
                (kit.probe)(&rep.old),
                rep.previous
            ),
        ),
        Err(e) => v(Rule::Replace, format!("refused: {e}")),
    }
    if probe(&r) != ["Z", "A", "B2", "C"] {
        v(Rule::Replace, format!("after replace: {:?}", probe(&r)));
    }

    // A second plugin replacing the same item is a conflict naming both.
    match r.replace(&two, b, (kit.make)("B3")) {
        Err(e)
            if e.code().as_str() == "PLUGIN-0006"
                && e.to_string().contains(one.as_str())
                && e.to_string().contains(two.as_str()) => {}
        Err(e) => v(Rule::Conflict, format!("wrong refusal: {e}")),
        Ok(_) => v(Rule::Conflict, "silently accepted (last-wins)".into()),
    }
    if probe(&r) != ["Z", "A", "B2", "C"] {
        v(
            Rule::Conflict,
            format!("after refused replace: {:?}", probe(&r)),
        );
    }

    // remove: gone, and handed back.
    match r.remove(&one, z) {
        Ok(item) if (kit.probe)(&item) == "Z" => {}
        Ok(item) => v(Rule::Remove, format!("returned {:?}", (kit.probe)(&item))),
        Err(e) => v(Rule::Remove, format!("refused: {e}")),
    }
    if probe(&r) != ["A", "B2", "C"] {
        v(Rule::Remove, format!("after remove: {:?}", probe(&r)));
    }

    // chain: wraps, keeps the original inside; chains compose.
    let wrap = kit.wrap;
    if let Err(e) = r.chain(&one, c, Box::new(wrap)) {
        v(Rule::Chain, format!("refused: {e}"));
    }
    if let Err(e) = r.chain(&two, a, Box::new(wrap)) {
        v(Rule::Chain, format!("refused: {e}"));
    }
    if let Err(e) = r.chain(&one, a, Box::new(wrap)) {
        v(Rule::Chain, format!("refused: {e}"));
    }
    if probe(&r) != ["wrap(wrap(A))", "B2", "wrap(C)"] {
        v(Rule::Chain, format!("after chains: {:?}", probe(&r)));
    }

    // Operations on missing items are refused with the stable code.
    let unknown =
        |res: Result<(), PluginError>| matches!(res, Err(e) if e.code().as_str() == "PLUGIN-0004");
    let ok = unknown(r.replace(&one, z, (kit.make)("Q")).map(|_| ()))
        && unknown(r.remove(&one, z).map(|_| ()))
        && unknown(r.chain(&one, z, Box::new(wrap)));
    if !ok {
        v(
            Rule::Unknown,
            "an operation on a removed item was not PLUGIN-0004".into(),
        );
    }
    out
}
