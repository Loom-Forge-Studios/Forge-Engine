//! `forge-graph` — blueprints, graph, IR, codegen (Ch.24).
//!
//! ```text
//!    visual graph  ─┐
//!    Rust source   ─┼─→  typed IR  ─→  Rust codegen  ─→  wasm (iterate) │ native (ship)
//!    automation    ─┘
//! ```
//!
//! * [`model`] — a graph as the IR reads it: nodes (an op, wired inputs, literals) of one
//!   [`GraphKind`]. The editor's graph documents convert to it; so can any other author.
//! * [`ir`] — the compiler: every wire type- and unit-checked through
//!   `forge_reflect::connect` (the same check the canvas makes while a wire is dragged),
//!   cycles, unknown ops, nodes a kind may not use, unconnected inputs, **ops in dependency
//!   order**, **readable Rust** that calls the nodes' functions, and diagnostics mapped back
//!   to nodes with the contract's `GRAPH-*` codes. [`ir::parse_text`] reads the mappable
//!   subset of that Rust back (graph ↔ text).
//! * [`library`] — [`library::Signatures`]: the nodes of `#[forge_api]` registries, for
//!   authors other than the editor (which has its own library with UI metadata and
//!   implements [`ir::NodeSource`] for it).
//! * Generator graphs (Ch.13) compile here like every graph and **lower to the plan of the
//!   generator crate that builds on this one** (this crate links no generator): the compiled
//!   ops are the generator's stage constructors, called in dependency order with their
//!   literals and wired stage values — exactly what the generated Rust does. The generator
//!   then runs natively from the plan; nothing interprets the graph while it runs (I10).
//!
//! Not built here (Spike S3, M5-4): building the generated Rust of a *blueprint* into wasm
//! (iteration) or native code (shipping) and loading it; gate row `C-graph-ir-backend` says so.

#![forbid(unsafe_code)]

pub mod ir;
pub mod library;
pub mod model;

pub use ir::{
    Compiled, Diagnostic, IrArg, IrOp, NodeSource, Sig, TextArg, TextGraph, TextNode, codes,
    compile, parse_text,
};
pub use model::{Graph, GraphKind, Node, REROUTE, Source};
