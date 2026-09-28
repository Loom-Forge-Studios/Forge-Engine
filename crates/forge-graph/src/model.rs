//! A graph as the IR reads it (Ch.24): nodes keyed by id, each an op, its wired inputs and
//! its literals, in a graph of one kind. Positions, comments and other canvas state are the
//! editor's and never reach the compiler.

use std::collections::BTreeMap;
use std::fmt;

use forge_cmd::Value;

/// The built-in reroute's op (not a registry item: a wire's waypoint, typed by its source).
pub const REROUTE: &str = "forge.reroute";

/// Which of the four graph users a graph is (Ch.24.1: one compiler, four users).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GraphKind {
    #[default]
    Blueprint,
    Material,
    Generator,
    Pcg,
}

impl GraphKind {
    pub const ALL: [GraphKind; 4] = [
        GraphKind::Blueprint,
        GraphKind::Material,
        GraphKind::Generator,
        GraphKind::Pcg,
    ];
    /// The stored name.
    pub fn name(self) -> &'static str {
        match self {
            GraphKind::Blueprint => "Blueprint",
            GraphKind::Material => "Material",
            GraphKind::Generator => "Generator",
            GraphKind::Pcg => "Pcg",
        }
    }
    pub fn parse(s: &str) -> Option<GraphKind> {
        GraphKind::ALL.into_iter().find(|k| k.name() == s)
    }
    /// What diagnostics and generated text call it.
    pub fn title(self) -> &'static str {
        match self {
            GraphKind::Blueprint => "Blueprint",
            GraphKind::Material => "Material graph",
            GraphKind::Generator => "Generator graph",
            GraphKind::Pcg => "PCG graph",
        }
    }
    /// The graph kinds a `#[forge_api]` category's items belong to: `Math` in every graph,
    /// `Material` / `Generator` / `PCG` in theirs, anything else in blueprints; a mutating
    /// item (a command) only in blueprints (the other three graphs are pure).
    pub fn for_category(category: &str, mutates: bool) -> Vec<GraphKind> {
        if mutates {
            return vec![GraphKind::Blueprint];
        }
        match category {
            "Math" => GraphKind::ALL.to_vec(),
            "Material" => vec![GraphKind::Material],
            "Generator" => vec![GraphKind::Generator],
            "PCG" => vec![GraphKind::Pcg],
            _ => vec![GraphKind::Blueprint],
        }
    }
}

/// Where an input's wire comes from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Source {
    pub node: u64,
    pub pin: String,
}

impl Source {
    pub fn new(node: u64, pin: &str) -> Self {
        Self {
            node,
            pin: pin.to_string(),
        }
    }
    /// `"3.return"`.
    pub fn parse(s: &str) -> Option<Source> {
        let (n, p) = s.split_once('.')?;
        let node = n.parse().ok()?;
        (!p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then(|| Source::new(node, p))
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.node, self.pin)
    }
}

/// One node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub op: String,
    /// Input pin → its wire's source.
    pub inputs: BTreeMap<String, Source>,
    /// Input pin → its literal (used while the input has no wire).
    pub literals: BTreeMap<String, Value>,
}

/// One graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub name: String,
    pub kind: GraphKind,
    pub nodes: BTreeMap<u64, Node>,
}

impl Graph {
    pub fn new(name: &str, kind: GraphKind) -> Self {
        Self {
            name: name.to_string(),
            kind,
            nodes: BTreeMap::new(),
        }
    }
    /// Add a node; its id is one more than the largest.
    pub fn add(&mut self, op: &str) -> u64 {
        let id = self.nodes.keys().next_back().map_or(1, |k| k + 1);
        self.nodes.insert(
            id,
            Node {
                op: op.to_string(),
                ..Node::default()
            },
        );
        id
    }
    /// Wire `from.pin` into `to.input` (replacing its wire: an input has at most one).
    pub fn wire(&mut self, from: u64, pin: &str, to: u64, input: &str) {
        if let Some(n) = self.nodes.get_mut(&to) {
            n.inputs.insert(input.to_string(), Source::new(from, pin));
        }
    }
    /// Set an input's literal.
    pub fn literal(&mut self, node: u64, input: &str, v: Value) {
        if let Some(n) = self.nodes.get_mut(&node) {
            n.literals.insert(input.to_string(), v);
        }
    }
}
