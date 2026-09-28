//! The typed IR and its compiler (Ch.24.1).
//!
//! [`compile`] lowers a [`Graph`] to [`IrOp`]s in dependency order, generates the Rust text
//! for them, and reports [`Diagnostic`]s **mapped back to nodes**. [`parse_text`] reads the
//! mappable subset of that text back (graph ↔ text). Both read node signatures through
//! [`NodeSource`], so the editor's library (with its UI metadata) and
//! [`crate::library::Signatures`] (registries alone) compile the same way.
//!
//! Diagnostic codes are the contract's (`GRAPH-*`, allocated in `docs/error-codes.md`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use forge_cmd::Value;
use forge_reflect::{Pin, PinKind, Purity, ReflectError};

use crate::model::{Graph, GraphKind, REROUTE, Source};

/// The contract's diagnostic codes.
pub mod codes {
    /// A node's op is not in the library.
    pub const UNKNOWN_OP: &str = "GRAPH-0001";
    /// An input has no wire and no literal, and its type has no default.
    pub const UNCONNECTED: &str = "GRAPH-0002";
    /// A wire (or literal) of the wrong type.
    pub const TYPE: &str = "GRAPH-0003";
    /// A wire between units of different dimensions.
    pub const UNIT: &str = "GRAPH-0004";
    /// Nodes that feed themselves.
    pub const CYCLE: &str = "GRAPH-0005";
    /// A node this kind of graph may not use (a command in a pure graph).
    pub const NOT_IN_KIND: &str = "GRAPH-0006";
    /// A wire from a node or pin that does not exist.
    pub const DANGLING: &str = "GRAPH-0007";
    /// Graph text that does not parse.
    pub const TEXT: &str = "GRAPH-0008";
    /// A generator graph that does not lower to a solve plan (no single `global_solve`
    /// node, or a node the lowering has no stage for).
    pub const NOT_LOWERABLE: &str = "GRAPH-0009";
}

/// One problem, mapped back to a node (and pin) where it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub node: Option<u64>,
    pub pin: Option<String>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code)?;
        if let Some(n) = self.node {
            write!(f, "node {n}: ")?;
        }
        f.write_str(&self.message)
    }
}

/// An IR op's argument.
#[derive(Clone, Debug, PartialEq)]
pub enum IrArg {
    /// Another op's value, times a unit conversion (1 when the units are equal).
    Node {
        node: u64,
        scale: f64,
    },
    Literal(Value),
    /// A context the call runs in (`world`).
    Context(String),
}

/// One op: a node's call.
#[derive(Clone, Debug, PartialEq)]
pub struct IrOp {
    pub node: u64,
    pub op: String,
    pub args: Vec<IrArg>,
}

/// What a compile produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compiled {
    /// Ops in dependency order.
    pub ops: Vec<IrOp>,
    /// The generated Rust.
    pub text: String,
    /// For each line of `text`, the node it is.
    pub lines: Vec<Option<u64>>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Compiled {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
    /// The line (0-based) of `node`'s op in the text.
    pub fn line_of(&self, node: u64) -> Option<usize> {
        self.lines.iter().position(|l| *l == Some(node))
    }
    /// Each node's diagnostics, joined (what the canvas shows on the node).
    pub fn errors_by_node(&self) -> BTreeMap<u64, String> {
        let mut out: BTreeMap<u64, String> = BTreeMap::new();
        for d in &self.diagnostics {
            if let Some(n) = d.node {
                let e = out.entry(n).or_default();
                if !e.is_empty() {
                    e.push_str("; ");
                }
                e.push_str(&format!("{} {}", d.code, d.message));
            }
        }
        out
    }
}

/// An argument read from graph text.
#[derive(Clone, Debug, PartialEq)]
pub enum TextArg {
    /// `n3` (or `n3 * 1000.0`: the conversion is recomputed from the pins).
    Node(u64),
    Literal(Value),
}

/// A node read from graph text: its op and its positional arguments by input pin.
#[derive(Clone, Debug, PartialEq)]
pub struct TextNode {
    pub op: String,
    pub args: BTreeMap<String, TextArg>,
    /// The text line it came from (0-based).
    pub line: usize,
}

/// Graph text, read back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextGraph {
    pub nodes: BTreeMap<u64, TextNode>,
}

/// A node's signature as the compiler reads it.
#[derive(Clone, Copy, Debug)]
pub struct Sig<'a> {
    pub title: &'a str,
    pub category: &'a str,
    pub inputs: &'a [Pin],
    pub outputs: &'a [Pin],
    pub purity: Purity,
    /// Context parameters the call takes first (`world`).
    pub context: &'a [String],
    /// The graph kinds that may use it.
    pub kinds: &'a [GraphKind],
}

impl Sig<'_> {
    pub fn input(&self, name: &str) -> Option<&Pin> {
        self.inputs.iter().find(|p| p.name == name)
    }
    pub fn output(&self, name: &str) -> Option<&Pin> {
        self.outputs.iter().find(|p| p.name == name)
    }
}

/// Where node signatures come from (the editor's library, or [`crate::library::Signatures`]).
pub trait NodeSource {
    /// The signature of `op` (an item path), if the source has it.
    fn sig(&self, op: &str) -> Option<Sig<'_>>;
    /// The op an identifier in generated text calls (`None`: unknown or ambiguous).
    fn op_of_ident(&self, ident: &str) -> Option<&str>;
    /// What generated text calls `op` (its identifier when unambiguous, else its path).
    fn call_name<'a>(&'a self, op: &'a str) -> &'a str;
}

/// The literal a scalar input takes when it has neither wire nor literal.
pub fn default_literal(k: PinKind) -> Option<Value> {
    match k {
        PinKind::Float => Some(Value::Float(0.0)),
        PinKind::Int => Some(Value::Int(0)),
        PinKind::Bool => Some(Value::Bool(false)),
        PinKind::Text => Some(Value::Text(String::new())),
        _ => None,
    }
}

/// Can `v` be the literal of a `k` input?
pub fn literal_fits(k: PinKind, v: &Value) -> bool {
    matches!(
        (k, v),
        (PinKind::Float, Value::Float(_) | Value::Int(_))
            | (PinKind::Int, Value::Int(_))
            | (PinKind::Bool, Value::Bool(_))
            | (PinKind::Text, Value::Text(_))
            | (PinKind::Vector, Value::Vec3(_))
            | (PinKind::Entity, Value::Entity(_))
    )
}

/// A literal as Rust source.
pub fn literal_text(v: &Value) -> String {
    match v {
        Value::Float(f) => format!("{f:?}"),
        Value::Int(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Text(t) => format!("{t:?}"),
        Value::Vec3(a) => format!("[{:?}, {:?}, {:?}]", a[0], a[1], a[2]),
        Value::Entity(e) => format!("EntityKey({})", e.0),
    }
}

/// A pin's type and unit as the canvas shows it (`f64 · m/s`).
pub fn pin_detail(p: &Pin) -> String {
    let ty = p.type_path.rsplit("::").next().unwrap_or(p.type_path);
    match &p.unit {
        Some(u) => format!("{ty} · {}", u.text()),
        None => ty.to_string(),
    }
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
            pin_detail(from),
            to.name,
            pin_detail(to)
        ),
    }
}

/// The output pin a wire's source names, following reroutes (`Ok(None)`: a reroute with
/// nothing wired into it).
pub fn output_pin<'a, L: NodeSource + ?Sized>(
    g: &Graph,
    lib: &'a L,
    src: &Source,
) -> Result<Option<&'a Pin>, String> {
    let mut cur = src.clone();
    for _ in 0..=g.nodes.len() {
        let n = g
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
        let ln = lib.sig(&n.op).ok_or_else(|| {
            format!(
                "node {} is `{}`, which no library item provides",
                cur.node, n.op
            )
        })?;
        return ln
            .outputs
            .iter()
            .find(|p| p.name == cur.pin)
            .map(Some)
            .ok_or_else(|| format!("{} has no output `{}`", ln.title, cur.pin));
    }
    Err("the reroutes form a loop".into())
}

fn snake(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        format!("graph_{s}")
    } else {
        s
    }
}

/// Compile a graph (see the module docs).
pub fn compile<L: NodeSource + ?Sized>(g: &Graph, lib: &L) -> Compiled {
    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut ops: BTreeMap<u64, IrOp> = BTreeMap::new();
    let mut deps: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    let d = |code, node: u64, pin: Option<&str>, message: String| Diagnostic {
        code,
        node: Some(node),
        pin: pin.map(str::to_string),
        message,
    };
    let kind_noun = g.kind.title().to_lowercase();
    for (&id, n) in &g.nodes {
        let mut args = Vec::new();
        let dep = deps.entry(id).or_default();
        if n.op == REROUTE {
            match n.inputs.get("in") {
                Some(s) if g.nodes.contains_key(&s.node) => {
                    dep.insert(s.node);
                    args.push(IrArg::Node {
                        node: s.node,
                        scale: 1.0,
                    });
                }
                Some(s) => diags.push(d(
                    codes::DANGLING,
                    id,
                    Some("in"),
                    format!(
                        "the reroute's wire comes from node {}, which does not exist",
                        s.node
                    ),
                )),
                None => diags.push(d(
                    codes::UNCONNECTED,
                    id,
                    Some("in"),
                    "the reroute has nothing wired into it".into(),
                )),
            }
            ops.insert(
                id,
                IrOp {
                    node: id,
                    op: REROUTE.into(),
                    args,
                },
            );
            continue;
        }
        let Some(ln) = lib.sig(&n.op) else {
            diags.push(d(
                codes::UNKNOWN_OP,
                id,
                None,
                format!("`{}` is not a node the library provides", n.op),
            ));
            ops.insert(
                id,
                IrOp {
                    node: id,
                    op: n.op.clone(),
                    args,
                },
            );
            continue;
        };
        if !ln.kinds.contains(&g.kind) {
            diags.push(d(
                codes::NOT_IN_KIND,
                id,
                None,
                if ln.purity == Purity::Mutate {
                    format!(
                        "`{}` changes project state; a {kind_noun} is pure and cannot use it",
                        ln.title
                    )
                } else {
                    format!(
                        "`{}` is a {} node; a {kind_noun} cannot use it",
                        ln.title, ln.category
                    )
                },
            ));
        }
        for c in ln.context {
            args.push(IrArg::Context(c.clone()));
        }
        for pin in ln.inputs {
            match n.inputs.get(pin.name) {
                Some(src) => {
                    if !g.nodes.contains_key(&src.node) {
                        diags.push(d(
                            codes::DANGLING,
                            id,
                            Some(pin.name),
                            format!(
                                "`{}` is wired from node {}, which does not exist",
                                pin.name, src.node
                            ),
                        ));
                        continue;
                    }
                    dep.insert(src.node);
                    let scale = match output_pin(g, lib, src) {
                        Ok(Some(o)) => match forge_reflect::connect(o, pin) {
                            Ok(c) => c.scale,
                            Err(e) => {
                                let code = match e {
                                    ReflectError::IncompatibleUnits { .. } => codes::UNIT,
                                    _ => codes::TYPE,
                                };
                                diags.push(d(code, id, Some(pin.name), refusal(o, pin, &e)));
                                1.0
                            }
                        },
                        Ok(None) => 1.0,
                        Err(why) => {
                            diags.push(d(codes::DANGLING, id, Some(pin.name), why));
                            1.0
                        }
                    };
                    args.push(IrArg::Node {
                        node: src.node,
                        scale,
                    });
                }
                None => match n.literals.get(pin.name) {
                    Some(v) if literal_fits(pin.kind, v) => args.push(IrArg::Literal(v.clone())),
                    Some(v) => diags.push(d(
                        codes::TYPE,
                        id,
                        Some(pin.name),
                        format!(
                            "`{}` takes {}, but its literal is {}",
                            pin.name,
                            pin_detail(pin),
                            v.kind()
                        ),
                    )),
                    None => match default_literal(pin.kind) {
                        Some(v) => args.push(IrArg::Literal(v)),
                        None => diags.push(d(
                            codes::UNCONNECTED,
                            id,
                            Some(pin.name),
                            format!("input `{}` is not connected", pin.name),
                        )),
                    },
                },
            }
        }
        for pin in n.inputs.keys() {
            if ln.input(pin).is_none() {
                diags.push(d(
                    codes::DANGLING,
                    id,
                    Some(pin),
                    format!("`{}` has no input `{pin}`", ln.title),
                ));
            }
        }
        ops.insert(
            id,
            IrOp {
                node: id,
                op: n.op.clone(),
                args,
            },
        );
    }
    // Dependency order (Kahn); what is left is in a cycle.
    let mut indeg: BTreeMap<u64, usize> = ops.keys().map(|k| (*k, 0)).collect();
    let mut users: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (n, ds) in &deps {
        for s in ds {
            if ops.contains_key(s) {
                *indeg.entry(*n).or_default() += 1;
                users.entry(*s).or_default().push(*n);
            }
        }
    }
    let mut ready: VecDeque<u64> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut order = Vec::new();
    while let Some(n) = ready.pop_front() {
        order.push(n);
        for u in users.get(&n).into_iter().flatten() {
            if let Some(d) = indeg.get_mut(u) {
                *d -= 1;
                if *d == 0 {
                    ready.push_back(*u);
                }
            }
        }
    }
    let placed: BTreeSet<u64> = order.iter().copied().collect();
    for n in ops
        .keys()
        .filter(|k| !placed.contains(k))
        .copied()
        .collect::<Vec<_>>()
    {
        diags.push(d(
            codes::CYCLE,
            n,
            None,
            "this node feeds itself through a cycle of wires".into(),
        ));
        order.push(n);
    }
    // The text.
    let mut text = String::new();
    let mut lines = Vec::new();
    let mut line = |text: &mut String, s: String, node: Option<u64>| {
        text.push_str(&s);
        text.push('\n');
        lines.push(node);
    };
    line(
        &mut text,
        format!(
            "// {} `{}`, compiled by forge-graph (Ch.24 IR).",
            g.kind.title(),
            g.name
        ),
        None,
    );
    let ctx: BTreeSet<String> = ops
        .values()
        .flat_map(|o| o.args.iter())
        .filter_map(|a| match a {
            IrArg::Context(c) => Some(c.clone()),
            _ => None,
        })
        .collect();
    let params: Vec<String> = ctx.iter().map(|c| format!("{c}: &mut World")).collect();
    line(
        &mut text,
        format!("pub fn {}({}) {{", snake(&g.name), params.join(", ")),
        None,
    );
    let mut out = Vec::new();
    for n in &order {
        let Some(op) = ops.remove(n) else { continue };
        let args: Vec<String> = op
            .args
            .iter()
            .map(|a| match a {
                IrArg::Node { node, scale } if (*scale - 1.0).abs() > f64::EPSILON => {
                    format!("n{node} * {scale:?}")
                }
                IrArg::Node { node, .. } => format!("n{node}"),
                IrArg::Literal(v) => literal_text(v),
                IrArg::Context(c) => c.clone(),
            })
            .collect();
        let s = if op.op == REROUTE {
            format!(
                "    let n{} = {}; // reroute",
                op.node,
                args.first().cloned().unwrap_or_else(|| "()".into())
            )
        } else {
            format!(
                "    let n{} = {}({});",
                op.node,
                lib.call_name(&op.op),
                args.join(", ")
            )
        };
        line(&mut text, s, Some(op.node));
        out.push(op);
    }
    line(&mut text, "}".into(), None);
    Compiled {
        ops: out,
        text,
        lines,
        diagnostics: diags,
    }
}

/// Read the mappable subset of graph text back (see the module docs).
pub fn parse_text<L: NodeSource + ?Sized>(
    text: &str,
    lib: &L,
) -> Result<TextGraph, Vec<Diagnostic>> {
    let mut g = TextGraph::default();
    let mut diags = Vec::new();
    let err = |line: usize, msg: String| Diagnostic {
        code: codes::TEXT,
        node: None,
        pin: None,
        message: format!("line {}: {msg}", line + 1),
    };
    for (i, raw) in text.lines().enumerate() {
        let l = strip_comment(raw).trim();
        if l.is_empty() || l == "}" || l.starts_with("pub fn ") || l.starts_with("fn ") {
            continue;
        }
        let Some(rest) = l.strip_prefix("let n") else {
            diags.push(err(i, format!("expected `let n<id> = ...;`, found `{l}`")));
            continue;
        };
        let Some((id, rhs)) = rest.split_once('=') else {
            diags.push(err(i, "expected `=`".into()));
            continue;
        };
        let Ok(id) = id.trim().parse::<u64>() else {
            diags.push(err(
                i,
                format!("`n{}` is not a node name (`n` and a number)", id.trim()),
            ));
            continue;
        };
        let Some(rhs) = rhs.trim().strip_suffix(';') else {
            diags.push(err(i, "expected `;` at the end".into()));
            continue;
        };
        let rhs = rhs.trim();
        if g.nodes.contains_key(&id) {
            diags.push(err(i, format!("`n{id}` is defined twice")));
            continue;
        }
        // `let n9 = n2;` is a reroute.
        if let Some(src) = node_ref(rhs) {
            g.nodes.insert(
                id,
                TextNode {
                    op: REROUTE.into(),
                    args: BTreeMap::from([("in".to_string(), TextArg::Node(src))]),
                    line: i,
                },
            );
            continue;
        }
        let Some((name, args)) = rhs.split_once('(') else {
            diags.push(err(i, format!("expected a call, found `{rhs}`")));
            continue;
        };
        let Some(args) = args.trim_end().strip_suffix(')') else {
            diags.push(err(i, "expected `)`".into()));
            continue;
        };
        let name = name.trim();
        let Some(op) = lib.op_of_ident(name) else {
            diags.push(err(
                i,
                format!("`{name}` is not a node the library provides"),
            ));
            continue;
        };
        let Some(ln) = lib.sig(op) else { continue };
        let parts = split_args(args);
        let skip = ln.context.len();
        if parts.len() != skip + ln.inputs.len() {
            diags.push(err(
                i,
                format!(
                    "`{name}` takes {} argument(s), found {}",
                    skip + ln.inputs.len(),
                    parts.len()
                ),
            ));
            continue;
        }
        let mut node = TextNode {
            op: op.to_string(),
            args: BTreeMap::new(),
            line: i,
        };
        let mut bad = false;
        for (pin, a) in ln.inputs.iter().zip(parts.iter().skip(skip)) {
            match parse_arg(a) {
                Some(v) => {
                    node.args.insert(pin.name.to_string(), v);
                }
                None => {
                    diags.push(err(
                        i,
                        format!("`{a}` is not a node, a number, a bool or a string"),
                    ));
                    bad = true;
                }
            }
        }
        if !bad {
            g.nodes.insert(id, node);
        }
    }
    for (id, n) in &g.nodes {
        for a in n.args.values() {
            if let TextArg::Node(s) = a
                && !g.nodes.contains_key(s)
            {
                diags.push(err(
                    n.line,
                    format!("`n{id}` uses `n{s}`, which is not defined"),
                ));
            }
        }
    }
    if diags.is_empty() { Ok(g) } else { Err(diags) }
}

fn strip_comment(l: &str) -> &str {
    let mut in_str = false;
    let b = l.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        match b[i] {
            b'"' => in_str = !in_str,
            b'\\' if in_str => i += 1,
            b'/' if !in_str && b[i + 1] == b'/' => return &l[..i],
            _ => {}
        }
        i += 1;
    }
    l
}

fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for c in s.chars() {
        if in_str {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `n3`, or `n3 * 1000.0` (a unit conversion, recomputed from the pins).
fn node_ref(s: &str) -> Option<u64> {
    let head = s.split('*').next()?.trim();
    head.strip_prefix('n')?.parse().ok()
}

fn parse_arg(s: &str) -> Option<TextArg> {
    if let Some(n) = node_ref(s) {
        return Some(TextArg::Node(n));
    }
    let v = match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ if s.starts_with('"') => Value::Text(serde_json::from_str::<String>(s).ok()?),
        _ if s.contains(['.', 'e', 'E']) || s.contains("inf") || s.contains("NaN") => {
            let f: f64 = s.parse().ok()?;
            if !f.is_finite() {
                return None;
            }
            Value::Float(f)
        }
        _ => Value::Int(s.parse().ok()?),
    };
    Some(TextArg::Literal(v))
}
