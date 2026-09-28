//! The graph editor's model (Ch.24; Ch.21 §21.21 "Graph editor", DoD M2-46, WP-U8): one
//! model for the four graph users — blueprints, material graphs (Ch.10), generator graphs
//! (Ch.13) and PCG graphs (Ch.14) — over the typed IR contract ([`ir`]).
//!
//! * **Project state is settings, edited by commands (I7).** A graph lives under
//!   `graph.<g>.` (see the table); every edit is `SetSetting` commands, several at once one
//!   transaction and one undo entry, so an automation session or a script can do all the editor
//!   does (the same model as the domain editors, ADR 0026; Ch.7 is frozen).
//! * **An input has at most one wire**, so a wire is a field of the input it feeds
//!   (`graph.g1.node.7.in.speed = "3.return"`), and connecting to a wired input replaces the
//!   old wire in the same transaction.
//! * **Pins are typed with units** from the node library ([`library`], every `#[forge_api]`
//!   item). [`check_connect`] refuses a wire whose types differ or whose units measure
//!   different things, and one that would close a cycle — always with a sentence saying why.
//! * **Followed incrementally.** [`Graphs::follow`] applies the changed setting keys to the
//!   nodes they touch (a 2,000-node graph is not re-read per edit), and re-reads everything
//!   only when it fell behind the change log.
//! * **Parsing is total.** A value of the wrong type (an automation session wrote it) is skipped
//!   and reported in the graph's problems, never a panic.
//!
//! | Key | Value |
//! |---|---|
//! | `graph.<g>.name` | text |
//! | `graph.<g>.kind` | text: `Blueprint`, `Material`, `Generator` or `Pcg` |
//! | `graph.<g>.node.<n>.op` | text: a library op (an item path, or `forge.reroute`) |
//! | `graph.<g>.node.<n>.x`, `.y` | float: top-left on the canvas |
//! | `graph.<g>.node.<n>.in.<pin>` | text: the source, `<node>.<pin>` |
//! | `graph.<g>.node.<n>.lit.<pin>` | any value: the input's literal when it has no wire |
//! | `graph.<g>.comment.<c>.x`, `.y`, `.w`, `.h` | float: the comment's frame |
//! | `graph.<g>.comment.<c>.text` | text |

pub mod ir;
pub mod library;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use forge_cmd::{EditorCommand, Value};
use forge_reflect::{Pin, ReflectError};

use crate::domain::{clear, set};
use crate::mirror::ProjectMirror;
use library::{LibNode, NodeLibrary, REROUTE};

/// The settings namespace.
pub const ROOT: &str = "graph";

/// Which of the four graph users a graph is.
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
    pub fn name(self) -> &'static str {
        match self {
            GraphKind::Blueprint => "Blueprint",
            GraphKind::Material => "Material",
            GraphKind::Generator => "Generator",
            GraphKind::Pcg => "Pcg",
        }
    }
    /// What the editor calls it.
    pub fn title(self) -> &'static str {
        match self {
            GraphKind::Blueprint => forge_ui::tr_key!("Blueprint"),
            GraphKind::Material => forge_ui::tr_key!("Material graph"),
            GraphKind::Generator => forge_ui::tr_key!("Generator graph"),
            GraphKind::Pcg => forge_ui::tr_key!("PCG graph"),
        }
    }
    pub fn parse(s: &str) -> Option<GraphKind> {
        GraphKind::ALL.into_iter().find(|k| k.name() == s)
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
pub struct GraphNode {
    pub id: u64,
    pub op: String,
    pub x: f64,
    pub y: f64,
    /// Input pin → its wire's source.
    pub inputs: BTreeMap<String, Source>,
    /// Input pin → its literal (used while the input has no wire).
    pub literals: BTreeMap<String, Value>,
    /// Values skipped while reading it.
    pub problems: Vec<String>,
}

/// One comment / group frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphComment {
    pub id: u64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: String,
}

/// One graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphDoc {
    pub id: String,
    pub name: String,
    pub kind: GraphKind,
    pub nodes: BTreeMap<u64, GraphNode>,
    pub comments: BTreeMap<u64, GraphComment>,
    /// Values skipped in the graph's own fields.
    pub header_problems: Vec<String>,
}

impl GraphDoc {
    /// Every value skipped while reading the graph.
    pub fn problems(&self) -> Vec<String> {
        let mut v = self.header_problems.clone();
        for n in self.nodes.values() {
            v.extend(n.problems.iter().cloned());
        }
        v
    }
    /// The next free node id.
    pub fn next_node_id(&self) -> u64 {
        self.nodes.keys().next_back().map_or(1, |k| k + 1)
    }
    pub fn next_comment_id(&self) -> u64 {
        self.comments.keys().next_back().map_or(1, |k| k + 1)
    }
    /// The nodes whose input is fed by `node` (any pin).
    pub fn consumers(&self, node: u64) -> impl Iterator<Item = (u64, &str)> + '_ {
        self.nodes.values().flat_map(move |n| {
            n.inputs
                .iter()
                .filter(move |(_, s)| s.node == node)
                .map(move |(p, _)| (n.id, p.as_str()))
        })
    }
    /// Does `from` feed `to`, directly or through other nodes? O(nodes + wires): the
    /// consumer lists are built once per question (a wire drag asks it for every pin it
    /// passes over).
    pub fn reaches(&self, from: u64, to: u64) -> bool {
        let mut out: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for n in self.nodes.values() {
            for s in n.inputs.values() {
                out.entry(s.node).or_default().push(n.id);
            }
        }
        let mut seen = BTreeSet::new();
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !seen.insert(n) {
                continue;
            }
            if let Some(next) = out.get(&n) {
                stack.extend(next.iter().copied());
            }
        }
        false
    }
}

/// Setting key of a graph field: `graph.<g>.<rest>`.
pub fn key(g: &str, rest: &str) -> String {
    format!("{ROOT}.{g}.{rest}")
}
/// Setting key of a node field: `graph.<g>.node.<n>.<field>`.
pub fn node_key(g: &str, n: u64, field: &str) -> String {
    format!("{ROOT}.{g}.node.{n}.{field}")
}
pub fn comment_key(g: &str, c: u64, field: &str) -> String {
    format!("{ROOT}.{g}.comment.{c}.{field}")
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    }
}

fn read_node(m: &ProjectMirror, g: &str, n: u64) -> Option<GraphNode> {
    let prefix = format!("{ROOT}.{g}.node.{n}.");
    let mut node = GraphNode {
        id: n,
        ..GraphNode::default()
    };
    let mut any = false;
    for (k, v) in m.settings_under(&prefix) {
        any = true;
        let field = &k[prefix.len()..];
        let at = || format!("{g}.node.{n}.{field}");
        match field {
            "op" => match v {
                Value::Text(t) => node.op = t.clone(),
                _ => node
                    .problems
                    .push(format!("{}: expected text, found {}", at(), v.kind())),
            },
            "x" | "y" => match num(v) {
                Some(x) if field == "x" => node.x = x,
                Some(y) => node.y = y,
                None => {
                    node.problems
                        .push(format!("{}: expected a number, found {}", at(), v.kind()))
                }
            },
            _ => {
                if let Some(pin) = field.strip_prefix("in.") {
                    match v {
                        Value::Text(t) => match Source::parse(t) {
                            Some(s) => {
                                node.inputs.insert(pin.to_string(), s);
                            }
                            None => node
                                .problems
                                .push(format!("{}: `{t}` is not `<node>.<pin>`", at())),
                        },
                        _ => node.problems.push(format!(
                            "{}: expected text, found {}",
                            at(),
                            v.kind()
                        )),
                    }
                } else if let Some(pin) = field.strip_prefix("lit.") {
                    node.literals.insert(pin.to_string(), v.clone());
                } else {
                    node.problems.push(format!("{}: unknown field", at()));
                }
            }
        }
    }
    if !any {
        return None;
    }
    if node.op.is_empty() {
        node.problems.push(format!("{g}.node.{n}: no op"));
    }
    Some(node)
}

fn read_comment(m: &ProjectMirror, g: &str, c: u64) -> Option<GraphComment> {
    let prefix = format!("{ROOT}.{g}.comment.{c}.");
    let mut out = GraphComment {
        id: c,
        w: 200.0,
        h: 120.0,
        ..GraphComment::default()
    };
    let mut any = false;
    for (k, v) in m.settings_under(&prefix) {
        any = true;
        match (&k[prefix.len()..], v) {
            ("text", Value::Text(t)) => out.text = t.clone(),
            ("x", v) => out.x = num(v).unwrap_or(out.x),
            ("y", v) => out.y = num(v).unwrap_or(out.y),
            ("w", v) => out.w = num(v).unwrap_or(out.w),
            ("h", v) => out.h = num(v).unwrap_or(out.h),
            _ => {}
        }
    }
    any.then_some(out)
}

/// What [`Graphs::follow`] changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changed {
    /// Everything was re-read (first read, or the change log was outrun).
    pub full: bool,
    /// Graphs whose name or kind changed, or which appeared or went.
    pub graphs: BTreeSet<String>,
    pub nodes: BTreeSet<(String, u64)>,
    pub comments: BTreeSet<(String, u64)>,
}

impl Changed {
    pub fn is_empty(&self) -> bool {
        !self.full && self.graphs.is_empty() && self.nodes.is_empty() && self.comments.is_empty()
    }
    /// Did anything in graph `g` change?
    pub fn touches(&self, g: &str) -> bool {
        self.full
            || self.graphs.contains(g)
            || self.nodes.iter().any(|(x, _)| x == g)
            || self.comments.iter().any(|(x, _)| x == g)
    }
}

/// Every graph in the project, followed incrementally (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct Graphs {
    pub graphs: BTreeMap<String, GraphDoc>,
    /// The mirror's setting-change sequence number this is up to date with.
    seen: Option<u64>,
    /// Node and comment reads while following (a probe for the incremental guard).
    pub reads: u64,
}

impl Graphs {
    /// Read everything.
    pub fn read(m: &ProjectMirror) -> Self {
        let mut g = Self::default();
        g.reread(m);
        g
    }

    fn reread(&mut self, m: &ProjectMirror) {
        self.graphs.clear();
        let mut ids: BTreeSet<String> = BTreeSet::new();
        let prefix = format!("{ROOT}.");
        for (k, _) in m.settings_under(&prefix) {
            if let Some((g, _)) = k[prefix.len()..].split_once('.') {
                ids.insert(g.to_string());
            }
        }
        for g in ids {
            self.reread_graph(m, &g);
        }
        self.seen = Some(m.setting_change_seq());
    }

    fn reread_graph(&mut self, m: &ProjectMirror, g: &str) {
        let prefix = format!("{ROOT}.{g}.");
        let mut nodes = BTreeSet::new();
        let mut comments = BTreeSet::new();
        for (k, _) in m.settings_under(&prefix) {
            let rest = &k[prefix.len()..];
            if let Some(r) = rest.strip_prefix("node.")
                && let Some(n) = r.split('.').next().and_then(|n| n.parse::<u64>().ok())
            {
                nodes.insert(n);
            } else if let Some(r) = rest.strip_prefix("comment.")
                && let Some(c) = r.split('.').next().and_then(|n| n.parse::<u64>().ok())
            {
                comments.insert(c);
            }
        }
        let mut doc = GraphDoc {
            id: g.to_string(),
            ..GraphDoc::default()
        };
        self.read_header(m, &mut doc);
        for n in nodes {
            self.reads += 1;
            if let Some(node) = read_node(m, g, n) {
                doc.nodes.insert(n, node);
            }
        }
        for c in comments {
            self.reads += 1;
            if let Some(cm) = read_comment(m, g, c) {
                doc.comments.insert(c, cm);
            }
        }
        self.graphs.insert(g.to_string(), doc);
    }

    fn read_header(&self, m: &ProjectMirror, doc: &mut GraphDoc) {
        doc.header_problems.clear();
        let g = doc.id.clone();
        doc.name = match m.setting(&key(&g, "name")) {
            Some(Value::Text(t)) => t.clone(),
            Some(v) => {
                doc.header_problems
                    .push(format!("{g}.name: expected text, found {}", v.kind()));
                g.clone()
            }
            None => g.clone(),
        };
        doc.kind = match m.setting(&key(&g, "kind")) {
            Some(Value::Text(t)) => GraphKind::parse(t).unwrap_or_else(|| {
                doc.header_problems
                    .push(format!("{g}.kind: `{t}` is not a graph kind"));
                GraphKind::Blueprint
            }),
            Some(v) => {
                doc.header_problems
                    .push(format!("{g}.kind: expected text, found {}", v.kind()));
                GraphKind::Blueprint
            }
            None => GraphKind::Blueprint,
        };
    }

    /// Bring the model up to date with the mirror: apply the changed keys to the nodes and
    /// comments they touch; re-read everything if the change log was outrun.
    pub fn follow(&mut self, m: &ProjectMirror) -> Changed {
        let mut ch = Changed::default();
        let Some(seen) = self.seen else {
            self.reread(m);
            ch.full = true;
            return ch;
        };
        let Some(keys) = m.setting_changes_since(seen) else {
            self.reread(m);
            ch.full = true;
            return ch;
        };
        let prefix = format!("{ROOT}.");
        for k in keys {
            let Some(rest) = k.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let Some((g, field)) = rest.split_once('.') else {
                continue;
            };
            let mut parts = field.splitn(3, '.');
            match (
                parts.next(),
                parts.next().and_then(|n| n.parse::<u64>().ok()),
            ) {
                (Some("node"), Some(n)) => {
                    ch.nodes.insert((g.to_string(), n));
                }
                (Some("comment"), Some(c)) => {
                    ch.comments.insert((g.to_string(), c));
                }
                _ => {
                    ch.graphs.insert(g.to_string());
                }
            }
        }
        self.seen = Some(m.setting_change_seq());
        for g in ch
            .graphs
            .iter()
            .chain(ch.nodes.iter().map(|(g, _)| g))
            .chain(ch.comments.iter().map(|(g, _)| g))
            .cloned()
            .collect::<BTreeSet<String>>()
        {
            let doc = self.graphs.entry(g.clone()).or_insert_with(|| GraphDoc {
                id: g.clone(),
                ..GraphDoc::default()
            });
            let mut d = std::mem::take(doc);
            if ch.graphs.contains(&g) || d.name.is_empty() {
                self.read_header(m, &mut d);
            }
            for (_, n) in ch.nodes.iter().filter(|(x, _)| *x == g) {
                self.reads += 1;
                match read_node(m, &g, *n) {
                    Some(node) => {
                        d.nodes.insert(*n, node);
                    }
                    None => {
                        d.nodes.remove(n);
                    }
                }
            }
            for (_, c) in ch.comments.iter().filter(|(x, _)| *x == g) {
                self.reads += 1;
                match read_comment(m, &g, *c) {
                    Some(cm) => {
                        d.comments.insert(*c, cm);
                    }
                    None => {
                        d.comments.remove(c);
                    }
                }
            }
            let gone = d.nodes.is_empty()
                && d.comments.is_empty()
                && m.setting(&key(&g, "name")).is_none()
                && m.setting(&key(&g, "kind")).is_none();
            if gone {
                self.graphs.remove(&g);
                ch.graphs.insert(g);
            } else if let Some(slot) = self.graphs.get_mut(&g) {
                *slot = d;
            }
        }
        ch
    }

    pub fn get(&self, g: &str) -> Option<&GraphDoc> {
        self.graphs.get(g)
    }

    /// A fresh graph id (`g1`, `g2`, ...).
    pub fn next_id(&self) -> String {
        let n = self
            .graphs
            .keys()
            .filter_map(|k| k.strip_prefix('g').and_then(|n| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        format!("g{}", n + 1)
    }
}

// ---- pins and the connection check -----------------------------------------------------------

/// The resolved type of a pin: its library pin, or for a reroute the pin at the end of its
/// chain (`None`: an unconnected reroute accepts anything).
pub fn output_pin<'a>(
    doc: &GraphDoc,
    lib: &'a NodeLibrary,
    src: &Source,
) -> Result<Option<&'a Pin>, String> {
    let mut cur = src.clone();
    for _ in 0..=doc.nodes.len() {
        let n = doc
            .nodes
            .get(&cur.node)
            .ok_or_else(|| format!("node {} does not exist", cur.node))?;
        if n.op == REROUTE {
            match n.inputs.get("in") {
                Some(s) => {
                    cur = s.clone();
                    continue;
                }
                None => return Ok(None),
            }
        }
        let ln = lib.get(&n.op).ok_or_else(|| {
            format!(
                "node {} is `{}`, which no library item provides",
                n.id, n.op
            )
        })?;
        return ln
            .output(&cur.pin)
            .map(Some)
            .ok_or_else(|| format!("{} has no output `{}`", ln.title, cur.pin));
    }
    Err("the reroutes form a loop".into())
}

/// The library pin an input is (`None`: a reroute's input, which takes anything).
pub fn input_pin<'a>(
    doc: &GraphDoc,
    lib: &'a NodeLibrary,
    node: u64,
    pin: &str,
) -> Result<Option<&'a Pin>, String> {
    let n = doc
        .nodes
        .get(&node)
        .ok_or_else(|| format!("node {node} does not exist"))?;
    if n.op == REROUTE {
        return if pin == "in" {
            Ok(None)
        } else {
            Err(format!("a reroute has no input `{pin}`"))
        };
    }
    let ln = lib
        .get(&n.op)
        .ok_or_else(|| format!("node {node} is `{}`, which no library item provides", n.op))?;
    ln.input(pin)
        .map(Some)
        .ok_or_else(|| format!("{} has no input `{pin}`", ln.title))
}

fn describe(p: &Pin) -> String {
    library::pin_detail(p)
}

/// Why two pins' values do not fit, in words (the reason the canvas shows).
pub fn refusal(from: &Pin, to: &Pin, e: &ReflectError) -> String {
    match (e, &from.unit, &to.unit) {
        (ReflectError::IncompatibleUnits { .. }, Some(a), Some(b)) => format!(
            "`{}` cannot feed `{}`: {} measures {} and {} measures {}; the units differ",
            from.name,
            to.name,
            a.text(),
            a.si_dimension(),
            b.text(),
            b.si_dimension()
        ),
        _ => format!(
            "`{}` ({}) cannot feed `{}` ({}): the types differ",
            from.name,
            describe(from),
            to.name,
            describe(to)
        ),
    }
}

/// The typed inputs that whatever enters reroute `reroute` finally reaches: its consumers,
/// followed through any further reroutes (each visited once, so a reroute loop ends).
fn typed_ends<'a>(doc: &GraphDoc, lib: &'a NodeLibrary, reroute: u64) -> Vec<&'a Pin> {
    let mut ends = Vec::new();
    let mut seen = BTreeSet::from([reroute]);
    let mut stack = vec![reroute];
    while let Some(r) = stack.pop() {
        for (c, pin) in doc.consumers(r) {
            match doc.nodes.get(&c) {
                Some(n) if n.op == REROUTE => {
                    if seen.insert(c) {
                        stack.push(c);
                    }
                }
                _ => {
                    if let Ok(Some(i)) = input_pin(doc, lib, c, pin) {
                        ends.push(i);
                    }
                }
            }
        }
    }
    ends
}

/// May the output `from` feed input `to_pin` of node `to`? `Ok(scale)`: the unit conversion
/// the wire applies (1 when equal). `Err`: why not, in a sentence.
pub fn check_connect(
    doc: &GraphDoc,
    lib: &NodeLibrary,
    from: &Source,
    to: u64,
    to_pin: &str,
) -> Result<f64, String> {
    if from.node == to {
        return Err("A node cannot feed itself.".into());
    }
    let out = output_pin(doc, lib, from)?;
    let inp = input_pin(doc, lib, to, to_pin)?;
    if doc.reaches(to, from.node) {
        return Err(format!(
            "This wire would make a cycle: node {to} already feeds node {}.",
            from.node
        ));
    }
    match (out, inp) {
        (Some(o), Some(i)) => forge_reflect::connect(o, i)
            .map(|c| c.scale)
            .map_err(|e| refusal(o, i, &e)),
        // A reroute on either side takes the type that reaches it.
        _ => {
            // A reroute's consumers must still accept what now flows through it — including
            // those behind further reroutes, so a chain of reroutes is checked end to end.
            if let (Some(o), None) = (out, inp) {
                for i in typed_ends(doc, lib, to) {
                    forge_reflect::connect(o, i).map_err(|e| refusal(o, i, &e))?;
                }
            }
            Ok(1.0)
        }
    }
}

// ---- commands ----------------------------------------------------------------------------------

/// Create a graph.
pub fn new_graph(gs: &Graphs, name: &str, kind: GraphKind) -> (String, Vec<EditorCommand>) {
    let id = gs.next_id();
    let cmds = vec![
        set(key(&id, "name"), Value::Text(name.to_string())),
        set(key(&id, "kind"), Value::Text(kind.name().to_string())),
    ];
    (id, cmds)
}

/// Remove a graph and everything in it.
pub fn delete_graph(m: &ProjectMirror, g: &str) -> Vec<EditorCommand> {
    crate::domain::clear_under(m, &format!("{ROOT}.{g}"))
}

/// Add node `id` running `op` at `(x, y)`.
pub fn add_node(g: &str, id: u64, op: &str, x: f64, y: f64) -> Vec<EditorCommand> {
    vec![
        set(node_key(g, id, "op"), Value::Text(op.to_string())),
        set(node_key(g, id, "x"), Value::Float(x)),
        set(node_key(g, id, "y"), Value::Float(y)),
    ]
}

/// Move nodes.
pub fn move_nodes(g: &str, moves: &[(u64, f64, f64)]) -> Vec<EditorCommand> {
    moves
        .iter()
        .flat_map(|(n, x, y)| {
            [
                set(node_key(g, *n, "x"), Value::Float(*x)),
                set(node_key(g, *n, "y"), Value::Float(*y)),
            ]
        })
        .collect()
}

/// Wire `from` into `to.pin` (replacing any wire there), after [`check_connect`].
pub fn connect(
    doc: &GraphDoc,
    lib: &NodeLibrary,
    from: &Source,
    to: u64,
    pin: &str,
) -> Result<Vec<EditorCommand>, String> {
    check_connect(doc, lib, from, to, pin)?;
    Ok(vec![set(
        node_key(&doc.id, to, &format!("in.{pin}")),
        Value::Text(from.to_string()),
    )])
}

/// Remove the wire into `to.pin`.
pub fn disconnect(g: &str, to: u64, pin: &str) -> Vec<EditorCommand> {
    vec![clear(node_key(g, to, &format!("in.{pin}")))]
}

/// Set (or clear) an input's literal.
pub fn set_literal(g: &str, node: u64, pin: &str, v: Option<Value>) -> EditorCommand {
    let k = node_key(g, node, &format!("lit.{pin}"));
    match v {
        Some(v) => set(k, v),
        None => clear(k),
    }
}

/// Delete nodes: their fields and every wire out of them.
pub fn delete_nodes(m: &ProjectMirror, doc: &GraphDoc, ids: &[u64]) -> Vec<EditorCommand> {
    let gone: BTreeSet<u64> = ids.iter().copied().collect();
    let mut cmds = Vec::new();
    for n in &gone {
        cmds.extend(crate::domain::clear_under(
            m,
            &format!("{ROOT}.{}.node.{n}", doc.id),
        ));
    }
    for node in doc.nodes.values().filter(|n| !gone.contains(&n.id)) {
        for (pin, s) in &node.inputs {
            if gone.contains(&s.node) {
                cmds.push(clear(node_key(&doc.id, node.id, &format!("in.{pin}"))));
            }
        }
    }
    cmds
}

/// Put a reroute (node `id`, at `(x, y)`) on the wire into `to.pin`.
pub fn insert_reroute(
    doc: &GraphDoc,
    to: u64,
    pin: &str,
    id: u64,
    x: f64,
    y: f64,
) -> Result<Vec<EditorCommand>, String> {
    let src = doc
        .nodes
        .get(&to)
        .and_then(|n| n.inputs.get(pin))
        .ok_or_else(|| format!("no wire goes into node {to}'s `{pin}`"))?;
    let g = &doc.id;
    let mut cmds = add_node(g, id, REROUTE, x, y);
    cmds.push(set(node_key(g, id, "in.in"), Value::Text(src.to_string())));
    cmds.push(set(
        node_key(g, to, &format!("in.{pin}")),
        Value::Text(Source::new(id, "out").to_string()),
    ));
    Ok(cmds)
}

/// Add a comment frame.
pub fn add_comment(g: &str, id: u64, rect: [f64; 4], text: &str) -> Vec<EditorCommand> {
    let mut v = set_comment_rect(g, id, rect);
    v.push(set(
        comment_key(g, id, "text"),
        Value::Text(text.to_string()),
    ));
    v
}

pub fn set_comment_rect(g: &str, id: u64, r: [f64; 4]) -> Vec<EditorCommand> {
    ["x", "y", "w", "h"]
        .iter()
        .zip(r)
        .map(|(f, v)| set(comment_key(g, id, f), Value::Float(v)))
        .collect()
}

pub fn delete_comment(m: &ProjectMirror, g: &str, id: u64) -> Vec<EditorCommand> {
    crate::domain::clear_under(m, &format!("{ROOT}.{g}.comment.{id}"))
}

// ---- copy / paste
// --------------------------------------------------------------------------------

/// What the clipboard carries for a copied selection: nodes (positions relative to the
/// selection's top-left), the wires between them, their literals and the comments.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fragment {
    pub nodes: Vec<FragNode>,
    pub comments: Vec<FragComment>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FragNode {
    pub id: u64,
    pub op: String,
    pub x: f64,
    pub y: f64,
    /// Wires from other copied nodes: `(input pin, source node, source pin)`.
    pub inputs: Vec<(String, u64, String)>,
    pub literals: Vec<(String, Value)>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FragComment {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: String,
}

/// The clipboard text's first line (what a paste recognises).
pub const CLIP_HEADER: &str = "forge-graph-fragment v1";

/// Copy nodes (and comments) to clipboard text.
pub fn copy(doc: &GraphDoc, nodes: &[u64], comments: &[u64]) -> Option<String> {
    let sel: BTreeSet<u64> = nodes
        .iter()
        .copied()
        .filter(|n| doc.nodes.contains_key(n))
        .collect();
    if sel.is_empty() && comments.is_empty() {
        return None;
    }
    let xs = sel
        .iter()
        .filter_map(|n| doc.nodes.get(n))
        .map(|n| (n.x, n.y))
        .chain(
            comments
                .iter()
                .filter_map(|c| doc.comments.get(c))
                .map(|c| (c.x, c.y)),
        );
    let (x0, y0) = xs.fold((f64::MAX, f64::MAX), |(a, b), (x, y)| (a.min(x), b.min(y)));
    let f = Fragment {
        nodes: sel
            .iter()
            .filter_map(|n| doc.nodes.get(n))
            .map(|n| FragNode {
                id: n.id,
                op: n.op.clone(),
                x: n.x - x0,
                y: n.y - y0,
                inputs: n
                    .inputs
                    .iter()
                    .filter(|(_, s)| sel.contains(&s.node))
                    .map(|(p, s)| (p.clone(), s.node, s.pin.clone()))
                    .collect(),
                literals: n
                    .literals
                    .iter()
                    .map(|(p, v)| (p.clone(), v.clone()))
                    .collect(),
            })
            .collect(),
        comments: comments
            .iter()
            .filter_map(|c| doc.comments.get(c))
            .map(|c| FragComment {
                x: c.x - x0,
                y: c.y - y0,
                w: c.w,
                h: c.h,
                text: c.text.clone(),
            })
            .collect(),
    };
    serde_json::to_string(&f)
        .ok()
        .map(|j| format!("{CLIP_HEADER}\n{j}"))
}

/// Paste clipboard text at `(x, y)`: new node ids, the copied wires remapped, one set of
/// commands. Returns the new node ids.
pub fn paste(
    doc: &GraphDoc,
    text: &str,
    x: f64,
    y: f64,
) -> Result<(Vec<u64>, Vec<EditorCommand>), String> {
    let json = text
        .strip_prefix(CLIP_HEADER)
        .ok_or("The clipboard does not hold graph nodes.")?;
    let f: Fragment = serde_json::from_str(json.trim())
        .map_err(|e| format!("The copied nodes do not parse: {e}"))?;
    let map: BTreeMap<u64, u64> = f
        .nodes
        .iter()
        .zip(doc.next_node_id()..)
        .map(|(n, id)| (n.id, id))
        .collect();
    let g = &doc.id;
    let mut cmds = Vec::new();
    let mut ids = Vec::new();
    for n in &f.nodes {
        let Some(&id) = map.get(&n.id) else { continue };
        ids.push(id);
        cmds.extend(add_node(g, id, &n.op, x + n.x, y + n.y));
        for (pin, src, spin) in &n.inputs {
            if let Some(s) = map.get(src) {
                cmds.push(set(
                    node_key(g, id, &format!("in.{pin}")),
                    Value::Text(Source::new(*s, spin).to_string()),
                ));
            }
        }
        for (pin, v) in &n.literals {
            cmds.push(set_literal(g, id, pin, Some(v.clone())));
        }
    }
    for (c, cm) in (doc.next_comment_id()..).zip(&f.comments) {
        cmds.extend(add_comment(
            g,
            c,
            [x + cm.x, y + cm.y, cm.w, cm.h],
            &cm.text,
        ));
    }
    Ok((ids, cmds))
}

/// A library node's pins as the canvas draws them.
pub fn canvas_pins(ln: &LibNode, output: bool) -> Vec<forge_ui::widgets::CanvasPin> {
    let pins = if output { &ln.outputs } else { &ln.inputs };
    pins.iter()
        .map(|p| {
            forge_ui::widgets::CanvasPin::new(
                p.meta.display_name.trim(),
                &library::pin_detail(p),
                library::pin_role(p.kind),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::BusClient;
    use crate::core::EditorCore;
    use forge_cmd::Issuer;

    fn lib() -> NodeLibrary {
        NodeLibrary::builtin(&[]).unwrap_or_else(|e| panic!("{e}"))
    }
    fn op(lib: &NodeLibrary, ident: &str) -> String {
        lib.op_of_ident(ident)
            .unwrap_or_else(|| panic!("{ident}"))
            .to_string()
    }

    /// A mirror kept up to date with a core, and a client applying commands.
    struct Fx {
        c: crate::core::LocalBus,
        m: ProjectMirror,
    }
    impl Fx {
        fn new() -> Self {
            let core = EditorCore::new();
            let c = EditorCore::connect(&core, Issuer::Test);
            let mut m = ProjectMirror::new();
            let (project, next) = c.snapshot();
            m.resync(&project, next, c.history());
            Self { c, m }
        }
        fn run(&mut self, cmds: Vec<EditorCommand>) {
            let t = self.c.begin("t");
            for c in cmds {
                self.c.apply(c, Some(t));
            }
            self.c.commit(t);
            let p = self.c.pump();
            for ev in &p.events {
                self.m.apply(ev, |t| self.c.txn_info(t));
            }
        }
    }

    #[test]
    fn a_graph_round_trips_through_settings_and_is_followed_incrementally() {
        let lib = lib();
        let mut fx = Fx::new();
        let mut gs = Graphs::read(&fx.m);
        let (g, cmds) = new_graph(&gs, "Motion", GraphKind::Blueprint);
        fx.run(cmds);
        fx.run(add_node(&g, 1, &op(&lib, "distance"), 10.0, 20.0));
        fx.run(add_node(&g, 2, &op(&lib, "speed"), 300.0, 20.0));
        let ch = gs.follow(&fx.m);
        assert!(ch.touches(&g));
        let doc = gs.get(&g).cloned().unwrap_or_default();
        assert_eq!(doc.name, "Motion");
        assert_eq!(doc.nodes.len(), 2);
        let cmds = connect(&doc, &lib, &Source::new(1, "return"), 2, "distance")
            .unwrap_or_else(|e| panic!("{e}"));
        fx.run(cmds);
        let reads = gs.reads;
        let ch = gs.follow(&fx.m);
        assert_eq!(
            ch.nodes,
            BTreeSet::from([(g.clone(), 2)]),
            "only node 2 changed"
        );
        assert_eq!(gs.reads - reads, 1, "one node re-read, not the graph");
        let doc = gs.get(&g).cloned().unwrap_or_default();
        assert_eq!(doc.nodes[&2].inputs["distance"], Source::new(1, "return"));
        assert_eq!(
            Graphs::read(&fx.m).get(&g),
            Some(&doc),
            "incremental == fresh"
        );
        // Deleting node 1 removes its wire into node 2 too.
        let cmds = delete_nodes(&fx.m, &doc, &[1]);
        fx.run(cmds);
        gs.follow(&fx.m);
        let doc = gs.get(&g).cloned().unwrap_or_default();
        assert!(!doc.nodes.contains_key(&1));
        assert!(doc.nodes[&2].inputs.is_empty());
        assert_eq!(Graphs::read(&fx.m).get(&g), Some(&doc));
    }

    #[test]
    fn connections_are_checked_by_type_unit_and_cycle() {
        let lib = lib();
        let mut doc = GraphDoc {
            id: "g1".into(),
            ..GraphDoc::default()
        };
        let mut put = |id: u64, ident: &str| {
            doc.nodes.insert(
                id,
                GraphNode {
                    id,
                    op: op(&lib, ident),
                    ..GraphNode::default()
                },
            );
        };
        put(1, "momentum"); // -> N·s
        put(2, "kinetic_energy"); // (kg, m/s) -> J
        put(3, "speed_from_impulse"); // (N·s, kg) -> m/s
        put(4, "greater"); // -> bool
        put(5, "select");
        // N·s into m/s: refused, naming both units and what they measure.
        let e = check_connect(&doc, &lib, &Source::new(1, "return"), 2, "speed")
            .err()
            .unwrap_or_default();
        assert!(
            e.contains("N·s") && e.contains("m/s") && e.contains("units differ"),
            "{e}"
        );
        // N·s into N·s: fine.
        assert_eq!(
            check_connect(&doc, &lib, &Source::new(1, "return"), 3, "impulse"),
            Ok(1.0)
        );
        // bool into f64: refused as a type mismatch.
        let e = check_connect(&doc, &lib, &Source::new(4, "return"), 5, "if_true")
            .err()
            .unwrap_or_default();
        assert!(e.contains("types differ"), "{e}");
        assert_eq!(
            check_connect(&doc, &lib, &Source::new(4, "return"), 5, "condition"),
            Ok(1.0)
        );
        // A cycle: 3 -> 2 exists; 2 -> 3 would close one (types aside).
        doc.nodes
            .get_mut(&2)
            .map(|n| n.inputs.insert("speed".into(), Source::new(3, "return")));
        doc.nodes
            .get_mut(&1)
            .map(|n| n.inputs.insert("velocity".into(), Source::new(2, "return")));
        let e = check_connect(&doc, &lib, &Source::new(1, "return"), 3, "impulse")
            .err()
            .unwrap_or_default();
        assert!(e.contains("cycle"), "{e}");
        // A reroute takes its source's type: J through a reroute into kg is still refused.
        doc.nodes.insert(
            6,
            GraphNode {
                id: 6,
                op: op(&lib, "kinetic_energy"),
                ..GraphNode::default()
            },
        );
        doc.nodes.insert(
            9,
            GraphNode {
                id: 9,
                op: REROUTE.into(),
                inputs: BTreeMap::from([("in".into(), Source::new(6, "return"))]),
                ..GraphNode::default()
            },
        );
        let e = check_connect(&doc, &lib, &Source::new(9, "out"), 3, "mass")
            .err()
            .unwrap_or_default();
        assert!(e.contains("units differ"), "{e}");
    }

    #[test]
    fn feeding_a_reroute_chain_checks_every_typed_end() {
        let lib = lib();
        let node = |id: u64, op_: String, inputs: &[(&str, Source)]| GraphNode {
            id,
            op: op_,
            inputs: inputs
                .iter()
                .map(|(p, s)| ((*p).to_string(), s.clone()))
                .collect(),
            ..GraphNode::default()
        };
        let mut doc = GraphDoc {
            id: "g1".into(),
            ..GraphDoc::default()
        };
        for n in [
            node(1, op(&lib, "momentum"), &[]), // -> N·s
            node(
                2,
                op(&lib, "kinetic_energy"),
                &[("speed", Source::new(12, "out"))],
            ), // m/s
            node(
                3,
                op(&lib, "speed_from_impulse"),
                &[("impulse", Source::new(11, "out"))],
            ), // N·s
            // R1 -> R2 -> R3 -> kinetic_energy.speed, and R2 -> speed_from_impulse.impulse.
            node(10, REROUTE.into(), &[]),
            node(11, REROUTE.into(), &[("in", Source::new(10, "out"))]),
            node(12, REROUTE.into(), &[("in", Source::new(11, "out"))]),
        ] {
            doc.nodes.insert(n.id, n);
        }
        // N·s into R1 would reach m/s three reroutes later: refused with the reason.
        let e = check_connect(&doc, &lib, &Source::new(1, "return"), 10, "in")
            .err()
            .unwrap_or_default();
        assert!(
            e.contains("N·s") && e.contains("m/s") && e.contains("units differ"),
            "{e}"
        );
        // Positive control: cut the m/s end off and the same wire is accepted (N·s -> N·s).
        if let Some(n) = doc.nodes.get_mut(&2) {
            n.inputs.clear();
        }
        assert_eq!(
            check_connect(&doc, &lib, &Source::new(1, "return"), 10, "in"),
            Ok(1.0)
        );
        // A loop among reroutes is walked once, not forever.
        doc.nodes
            .get_mut(&10)
            .map(|n| n.inputs.insert("in".into(), Source::new(12, "out")));
        assert!(typed_ends(&doc, &lib, 10).len() == 1);
    }

    #[test]
    fn copy_then_paste_remaps_ids_and_keeps_internal_wires_only() {
        let lib = lib();
        let mut doc = GraphDoc {
            id: "g1".into(),
            ..GraphDoc::default()
        };
        for (id, x) in [(1u64, 0.0), (2, 200.0), (3, 400.0)] {
            doc.nodes.insert(
                id,
                GraphNode {
                    id,
                    op: op(&lib, "add"),
                    x,
                    y: 50.0,
                    ..GraphNode::default()
                },
            );
        }
        doc.nodes
            .get_mut(&2)
            .map(|n| n.inputs.insert("a".into(), Source::new(1, "return")));
        doc.nodes
            .get_mut(&3)
            .map(|n| n.inputs.insert("a".into(), Source::new(2, "return")));
        doc.nodes
            .get_mut(&3)
            .map(|n| n.literals.insert("b".into(), Value::Float(2.5)));
        let text = copy(&doc, &[2, 3], &[]).unwrap_or_default();
        let (ids, cmds) = paste(&doc, &text, 1000.0, 1000.0).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ids, vec![4, 5]);
        let has = |k: &str, v: Value| cmds.contains(&set(k.to_string(), v));
        assert!(has("graph.g1.node.4.x", Value::Float(1000.0)));
        assert!(has("graph.g1.node.5.x", Value::Float(1200.0)));
        assert!(has("graph.g1.node.5.in.a", Value::Text("4.return".into())));
        assert!(has("graph.g1.node.5.lit.b", Value::Float(2.5)));
        assert!(
            !cmds.iter().any(|c| matches!(c, EditorCommand::SetSetting { key, .. } if key == "graph.g1.node.4.in.a")),
            "the wire from node 1 (not copied) is not pasted"
        );
        assert!(paste(&doc, "hello", 0.0, 0.0).is_err());
    }
}
