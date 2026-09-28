//! The **typed IR contract** the graph editor compiles through (Ch.24.1: "visual graph,
//! Rust source, automation session → typed IR → Rust codegen").
//!
//! [`GraphIr::compile`] lowers a graph to IR ops in dependency order, generates the Rust
//! text for them, and reports [`Diagnostic`]s **mapped back to nodes** (the editor draws
//! them on the nodes and in the console). [`GraphIr::parse_text`] reads the mappable subset
//! of that text back (graph ↔ text round trip), and [`apply_text`] turns the difference into
//! commands (I7).
//!
//! [`ForgeGraphIr`] is the implementation: **`forge-graph`** (WP-23, ADR 0049). The editor's
//! graph document converts to `forge_graph::Graph` (canvas state stays behind) and the
//! [`NodeLibrary`] is its node source, so the canvas's wire check and the compiler read the
//! same pins. A generator graph also **lowers** ([`lower_generator`]) to the
//! work it runs, through a [`GeneratorBackend`] a plugin provides (the editor links no
//! generator itself). What is still not built —
//! compiling a blueprint's generated Rust to wasm or native code and loading it (Spike S3) — is
//! listed by `just gate` (row `C-graph-ir-backend`).
//!
//! Diagnostic codes are the contract's (`GRAPH-*`, allocated in `docs/error-codes.md`).

use forge_cmd::EditorCommand;
pub use forge_graph::ir::{
    Compiled, Diagnostic, IrArg, IrOp, TextArg, TextGraph, TextNode, codes, literal_text,
};
use forge_graph::ir::{Sig, default_literal};
use forge_graph::{Graph, NodeSource};

use super::library::{NodeLibrary, REROUTE};
use super::{GraphDoc, GraphKind, Source, add_node, input_pin, node_key, set_literal};
use crate::domain::{BackendInfo, clear, set};

/// The IR contract (see the module docs).
pub trait GraphIr {
    /// Which backend this is (the panel says what it builds and what it does not).
    fn backend(&self) -> BackendInfo;
    /// Lower a graph: IR ops, generated Rust, diagnostics mapped to nodes.
    fn compile(&self, doc: &GraphDoc, lib: &NodeLibrary) -> Compiled;
    /// Read the mappable subset of graph text back.
    fn parse_text(&self, text: &str, lib: &NodeLibrary) -> Result<TextGraph, Vec<Diagnostic>>;
}

/// The graph IR on `forge-graph` (see the module docs).
#[derive(Clone, Copy, Debug, Default)]
pub struct ForgeGraphIr;

impl GraphKind {
    /// The IR's kind.
    pub fn ir(self) -> forge_graph::GraphKind {
        match self {
            GraphKind::Blueprint => forge_graph::GraphKind::Blueprint,
            GraphKind::Material => forge_graph::GraphKind::Material,
            GraphKind::Generator => forge_graph::GraphKind::Generator,
            GraphKind::Pcg => forge_graph::GraphKind::Pcg,
        }
    }
}

/// The graph as the IR reads it (ops, wires, literals; canvas state stays behind).
pub fn ir_graph(doc: &GraphDoc) -> Graph {
    let mut g = Graph::new(&doc.name, doc.kind.ir());
    for (id, n) in &doc.nodes {
        g.nodes.insert(
            *id,
            forge_graph::Node {
                op: n.op.clone(),
                inputs: n
                    .inputs
                    .iter()
                    .map(|(p, s)| (p.clone(), forge_graph::Source::new(s.node, &s.pin)))
                    .collect(),
                literals: n.literals.clone(),
            },
        );
    }
    g
}

impl NodeSource for NodeLibrary {
    fn sig(&self, op: &str) -> Option<Sig<'_>> {
        self.get(op).map(|n| Sig {
            title: &n.title,
            category: &n.category,
            inputs: &n.inputs,
            outputs: &n.outputs,
            purity: n.purity,
            context: &n.context,
            kinds: &n.ir_kinds,
        })
    }
    fn op_of_ident(&self, ident: &str) -> Option<&str> {
        NodeLibrary::op_of_ident(self, ident)
    }
    fn call_name<'a>(&'a self, op: &'a str) -> &'a str {
        NodeLibrary::call_name(self, op)
    }
}

impl GraphIr for ForgeGraphIr {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "forge-graph".into(),
            in_memory: false,
            note: "Compiled by forge-graph: wires are type- and unit-checked, ops ordered and \
                   Rust generated; a generator graph lowers to the work its backend runs. \
                   Building a blueprint's Rust to wasm or native code is not built yet (Spike S3)."
                .into(),
        }
    }

    fn compile(&self, doc: &GraphDoc, lib: &NodeLibrary) -> Compiled {
        forge_graph::compile(&ir_graph(doc), lib)
    }

    fn parse_text(&self, text: &str, lib: &NodeLibrary) -> Result<TextGraph, Vec<Diagnostic>> {
        forge_graph::parse_text(text, lib)
    }
}

/// What a generator graph lowers to, as the editor reports it: the work it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoweredGenerator {
    /// How many cells the work runs on.
    pub cells: usize,
}

/// A generator backend (Ch.13): the generator graph's stage nodes and the lowering of a
/// compiled generator graph to the work it describes. The editor links no generator: a
/// plugin provides one through [`GeneratorBackendPoint`]; with none loaded the library has no
/// stage nodes and nothing lowers.
pub trait GeneratorBackend: Send + Sync {
    /// The backend's name (a string key).
    fn name(&self) -> &str;
    /// The stage nodes' registry (they join the graph library).
    fn registry(&self) -> Result<forge_reflect::ForgeRegistry, String>;
    /// Compile `g` and lower it (`Err`: the compile's or the lowering's diagnostics).
    fn lower(&self, g: &Graph, lib: &NodeLibrary) -> Result<LoweredGenerator, Vec<Diagnostic>>;
}

/// The `GeneratorBackend` extension point (`forge.editor.generator_backend`, WP-23): a
/// plugin provides its generator's backend this way, keyed by the backend's name.
/// `shell::assemble_hosted` takes the first one loaded into
/// [`crate::services::EditorServices::generator`].
pub struct GeneratorBackendPoint;

impl forge_plugin::ExtensionPoint for GeneratorBackendPoint {
    type Item = std::sync::Arc<dyn GeneratorBackend>;
    const ID: &'static str = "forge.editor.generator_backend";
    const NAME: &'static str = "GeneratorBackend";
}

/// Compile a generator graph and lower it through `backend` to the work it runs
/// (`Err`: the compile's or the lowering's diagnostics, mapped to nodes).
pub fn lower_generator(
    doc: &GraphDoc,
    lib: &NodeLibrary,
    backend: &dyn GeneratorBackend,
) -> Result<LoweredGenerator, Vec<Diagnostic>> {
    backend.lower(&ir_graph(doc), lib)
}

/// The commands that make `doc` what `text` says: new nodes (placed right of the graph),
/// removed nodes, changed ops, wires and literals. Positions of existing nodes are kept.
pub fn apply_text(doc: &GraphDoc, lib: &NodeLibrary, text: &TextGraph) -> Vec<EditorCommand> {
    let g = &doc.id;
    let mut cmds = Vec::new();
    let right = doc.nodes.values().map(|n| n.x).fold(0.0f64, f64::max) + 260.0;
    let mut y = 0.0;
    let out_pin = |id: u64| -> String {
        let op = text
            .nodes
            .get(&id)
            .map(|n| n.op.as_str())
            .or_else(|| doc.nodes.get(&id).map(|n| n.op.as_str()))
            .unwrap_or_default();
        if op == REROUTE {
            "out".to_string()
        } else {
            lib.get(op)
                .and_then(|l| l.outputs.first())
                .map_or_else(|| "return".to_string(), |p| p.name.to_string())
        }
    };
    let gone: Vec<u64> = doc
        .nodes
        .keys()
        .filter(|k| !text.nodes.contains_key(k))
        .copied()
        .collect();
    for n in &gone {
        if let Some(node) = doc.nodes.get(n) {
            for f in ["op", "x", "y"] {
                cmds.push(clear(node_key(g, *n, f)));
            }
            for p in node.inputs.keys() {
                cmds.push(clear(node_key(g, *n, &format!("in.{p}"))));
            }
            for p in node.literals.keys() {
                cmds.push(clear(node_key(g, *n, &format!("lit.{p}"))));
            }
        }
    }
    for (id, tn) in &text.nodes {
        let existing = doc.nodes.get(id);
        match existing {
            None => {
                cmds.extend(add_node(g, *id, &tn.op, right, y));
                y += 140.0;
            }
            Some(n) if n.op != tn.op => {
                cmds.push(set(
                    node_key(g, *id, "op"),
                    forge_cmd::Value::Text(tn.op.clone()),
                ));
            }
            Some(_) => {}
        }
        let pins: Vec<String> = if tn.op == REROUTE {
            vec!["in".into()]
        } else {
            lib.get(&tn.op)
                .map(|l| l.inputs.iter().map(|p| p.name.to_string()).collect())
                .unwrap_or_default()
        };
        for pin in pins {
            let cur_in = existing.and_then(|n| n.inputs.get(&pin));
            let cur_lit = existing.and_then(|n| n.literals.get(&pin));
            match tn.args.get(&pin) {
                Some(TextArg::Node(s)) => {
                    let src = Source::new(*s, &out_pin(*s));
                    if cur_in != Some(&src) {
                        cmds.push(set(
                            node_key(g, *id, &format!("in.{pin}")),
                            forge_cmd::Value::Text(src.to_string()),
                        ));
                    }
                }
                Some(TextArg::Literal(v)) => {
                    if cur_in.is_some() {
                        cmds.push(clear(node_key(g, *id, &format!("in.{pin}"))));
                    }
                    if cur_lit != Some(v) {
                        cmds.push(set_literal(g, *id, &pin, Some(v.clone())));
                    }
                }
                None => {
                    if cur_in.is_some() {
                        cmds.push(clear(node_key(g, *id, &format!("in.{pin}"))));
                    }
                }
            }
        }
    }
    cmds
}

/// Is `pin` an input of `node` that a literal can fill (a scalar with no wire)?
pub fn literal_pin(doc: &GraphDoc, lib: &NodeLibrary, node: u64, pin: &str) -> bool {
    matches!(input_pin(doc, lib, node, pin), Ok(Some(p)) if default_literal(p.kind).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphNode;
    use forge_cmd::Value;

    fn lib() -> NodeLibrary {
        NodeLibrary::builtin(&[]).unwrap_or_else(|e| panic!("{e}"))
    }

    fn node(lib: &NodeLibrary, id: u64, ident: &str) -> GraphNode {
        GraphNode {
            id,
            op: lib.op_of_ident(ident).unwrap_or(ident).to_string(),
            x: id as f64 * 200.0,
            ..GraphNode::default()
        }
    }

    fn motion(lib: &NodeLibrary) -> GraphDoc {
        let mut doc = GraphDoc {
            id: "g1".into(),
            name: "Motion".into(),
            kind: GraphKind::Blueprint,
            ..GraphDoc::default()
        };
        let mut d = node(lib, 1, "distance");
        d.literals.insert("speed".into(), Value::Float(12.5));
        d.literals.insert("time".into(), Value::Float(3.0));
        doc.nodes.insert(1, d);
        let mut s = node(lib, 2, "speed");
        s.inputs.insert("distance".into(), Source::new(1, "return"));
        s.literals.insert("time".into(), Value::Float(3.0));
        doc.nodes.insert(2, s);
        let mut r = node(lib, 9, REROUTE);
        r.op = REROUTE.into();
        r.inputs.insert("in".into(), Source::new(2, "return"));
        doc.nodes.insert(9, r);
        let mut k = node(lib, 3, "kinetic_energy");
        k.literals.insert("mass".into(), Value::Float(80.0));
        k.inputs.insert("speed".into(), Source::new(9, "out"));
        doc.nodes.insert(3, k);
        doc
    }

    #[test]
    fn a_clean_graph_lowers_in_dependency_order_to_readable_rust() {
        let lib = lib();
        let c = ForgeGraphIr.compile(&motion(&lib), &lib);
        assert!(c.ok(), "{:?}", c.diagnostics);
        let order: Vec<u64> = c.ops.iter().map(|o| o.node).collect();
        assert_eq!(order, vec![1, 2, 9, 3]);
        assert!(c.text.contains("pub fn motion() {"), "{}", c.text);
        assert!(
            c.text.contains("    let n1 = distance(12.5, 3.0);"),
            "{}",
            c.text
        );
        assert!(c.text.contains("    let n9 = n2; // reroute"), "{}", c.text);
        assert!(
            c.text.contains("    let n3 = kinetic_energy(80.0, n9);"),
            "{}",
            c.text
        );
        assert_eq!(c.lines[c.line_of(3).unwrap_or(0)], Some(3));
    }

    #[test]
    fn errors_map_back_to_the_nodes_that_cause_them() {
        let lib = lib();
        let mut doc = motion(&lib);
        // An N·s output into the m/s input (written by an automation session, past the canvas's
        // check).
        let mut m = node(&lib, 4, "momentum");
        m.literals.insert("mass".into(), Value::Float(1.0));
        m.literals.insert("velocity".into(), Value::Float(1.0));
        doc.nodes.insert(4, m);
        doc.nodes
            .get_mut(&3)
            .map(|n| n.inputs.insert("speed".into(), Source::new(4, "return")));
        doc.nodes.insert(5, node(&lib, 5, "no_such_node"));
        let mut a = node(&lib, 6, "add");
        a.inputs.insert("a".into(), Source::new(7, "return"));
        let mut b = node(&lib, 7, "add");
        b.inputs.insert("a".into(), Source::new(6, "return"));
        doc.nodes.insert(6, a);
        doc.nodes.insert(7, b);
        doc.nodes.insert(8, node(&lib, 8, "schlick_fresnel"));
        let c = ForgeGraphIr.compile(&doc, &lib);
        let e = c.errors_by_node();
        assert!(
            e[&3].contains(codes::UNIT) && e[&3].contains("N·s"),
            "{e:?}"
        );
        assert!(e[&5].contains(codes::UNKNOWN_OP), "{e:?}");
        assert!(
            e[&6].contains(codes::CYCLE) && e[&7].contains(codes::CYCLE),
            "{e:?}"
        );
        assert!(e[&8].contains(codes::NOT_IN_KIND), "{e:?}");
        assert!(!e.contains_key(&1) && !e.contains_key(&2), "{e:?}");
    }

    #[test]
    fn the_generated_text_reads_back_to_the_same_graph() {
        let lib = lib();
        let doc = motion(&lib);
        let c = ForgeGraphIr.compile(&doc, &lib);
        let tg = ForgeGraphIr
            .parse_text(&c.text, &lib)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            apply_text(&doc, &lib, &tg).is_empty(),
            "a round trip changes nothing"
        );
        // Edit the text: a new node, a changed literal, a rewired input, a removed reroute.
        let edited = c
            .text
            .replace("distance(12.5, 3.0)", "distance(20.0, 3.0)")
            .replace("    let n9 = n2; // reroute\n", "")
            .replace("kinetic_energy(80.0, n9)", "kinetic_energy(80.0, n2)")
            .replace("}\n", "    let n10 = add(n3, 1.0);\n}\n");
        let tg = ForgeGraphIr
            .parse_text(&edited, &lib)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let cmds = apply_text(&doc, &lib, &tg);
        let has = |k: &str, v: Value| cmds.contains(&set(k.to_string(), v));
        assert!(has("graph.g1.node.1.lit.speed", Value::Float(20.0)));
        assert!(has(
            "graph.g1.node.3.in.speed",
            Value::Text("2.return".into())
        ));
        assert!(cmds.contains(&clear("graph.g1.node.9.op")));
        assert!(has("graph.g1.node.10.in.a", Value::Text("3.return".into())));
        assert!(has("graph.g1.node.10.lit.b", Value::Float(1.0)));
        // Bad text is refused line by line, with nothing applied.
        let e = ForgeGraphIr
            .parse_text(
                "pub fn x() {\n    let n1 = nope(1.0);\n    let q = 3;\n}\n",
                &lib,
            )
            .err()
            .unwrap_or_default();
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e.iter().all(|d| d.code == codes::TEXT));
        assert!(e[0].message.starts_with("line 2"), "{e:?}");
    }
}
