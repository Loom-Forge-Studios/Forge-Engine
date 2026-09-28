//! [`Signatures`]: every item of the given `#[forge_api]` registries as a node signature
//! (Ch.24.2 — "every `#[forge_api]` item is a node"), for authors that compile graphs without
//! the editor (tests, tools, an automation session's batch). The editor's own library carries UI
//! metadata besides and implements [`NodeSource`] itself; both read the same registry descriptors,
//! so a graph compiles the same through either.

use std::collections::BTreeMap;

use forge_reflect::{ForgeRegistry, Pin, Purity};

use crate::ir::{NodeSource, Sig};
use crate::model::GraphKind;

#[derive(Clone, Debug)]
struct Owned {
    ident: String,
    title: String,
    category: String,
    inputs: Vec<Pin>,
    outputs: Vec<Pin>,
    purity: Purity,
    context: Vec<String>,
    kinds: Vec<GraphKind>,
}

/// Node signatures from registries.
#[derive(Clone, Debug, Default)]
pub struct Signatures {
    nodes: BTreeMap<String, Owned>,
    /// ident → op, for idents no two items share.
    by_ident: BTreeMap<String, Option<String>>,
}

impl Signatures {
    pub fn from_registries(regs: &[&ForgeRegistry]) -> Self {
        let mut s = Self::default();
        for reg in regs {
            for item in reg.items() {
                let n = &item.node;
                let category = n.category.unwrap_or("Blueprint").to_string();
                let ident = item.ident.to_string();
                let e = s.by_ident.entry(ident.clone()).or_insert(None);
                *e = match e {
                    None if !s.nodes.values().any(|o| o.ident == ident) => Some(n.path.clone()),
                    _ => None,
                };
                s.nodes.insert(
                    n.path.clone(),
                    Owned {
                        ident,
                        title: n.display_name.to_string(),
                        kinds: GraphKind::for_category(&category, n.purity == Purity::Mutate),
                        category,
                        inputs: n.inputs.clone(),
                        outputs: n.outputs.clone(),
                        purity: n.purity,
                        context: n.context.iter().map(|c| c.name.to_string()).collect(),
                    },
                );
            }
        }
        s
    }
    /// Every op (item path).
    pub fn ops(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl NodeSource for Signatures {
    fn sig(&self, op: &str) -> Option<Sig<'_>> {
        self.nodes.get(op).map(|o| Sig {
            title: &o.title,
            category: &o.category,
            inputs: &o.inputs,
            outputs: &o.outputs,
            purity: o.purity,
            context: &o.context,
            kinds: &o.kinds,
        })
    }
    fn op_of_ident(&self, ident: &str) -> Option<&str> {
        if let Some((k, _)) = self.nodes.get_key_value(ident) {
            return Some(k.as_str());
        }
        self.by_ident.get(ident)?.as_deref()
    }
    fn call_name<'a>(&'a self, op: &'a str) -> &'a str {
        match self.nodes.get(op) {
            Some(n) if self.by_ident.get(&n.ident).is_some_and(Option::is_some) => &n.ident,
            _ => op,
        }
    }
}
