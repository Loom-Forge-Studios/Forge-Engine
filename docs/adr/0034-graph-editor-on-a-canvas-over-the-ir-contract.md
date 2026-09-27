# ADR 0034 — Build the graph editor as a node canvas widget over graph settings and the GraphIr contract

- **Status:** accepted
- **Date:** 2026-09-22
- **Plan references:** Ch.21 §21.16 (Graph row), §21.21 (Graph editor, `forge.graph`), §21.22
  (`ui_graph_2k_nodes`), Ch.24 (graph, IR, codegen), Ch.6 (unit-typed pins), I7, W5, D-4, D-5;
  DoD M2-46 (and the node canvas of M2-24, the graph half of M2-39)

## Context

WP-U8 asks for one graph editor for blueprints, material, generator and PCG graphs (E-11),
with unit-typed pins that refuse incompatible connections with a reason, node search from
the `#[forge_api]` registry, comments, reroutes, a minimap, copy/paste, compile errors on
nodes, a graph ↔ text view, edits as commands, and a 2,000-node graph that pans and zooms
within 4.0 ms. `forge-graph` (M5-4), which owns the IR and codegen, does not exist. The same
WP had to stop `test_ui_hierarchy_100k` failing whenever another lane builds beside it,
without widening its 2 ms budget.

## Decision

1. **The node canvas is a `forge-ui` catalogue widget** (`widgets/node_canvas.rs`) that
   knows nothing of graphs as data: the application sets nodes, pins, wires, comments and
   errors through `NodeCanvas::edit` and receives typed actions. It records only the nodes
   and wires in view, draws a node as one quad below `DETAIL_ZOOM`, strokes all wires of a
   colour as one polyline mesh (`MeshData::stroke_polyline`), keeps the minimap's every-node
   slice in a child widget that a pan never invalidates, and refreshes accessibility bounds
   once the view settles (one timer). It checks each candidate wire while it is dragged with
   the application's `ConnectCheck` and shows a refusal's reason beside the pin.
2. **A graph is project settings under `graph.<g>.`** — one value per node field; a wire is
   the field of the input it feeds (`node.<n>.in.<pin> = "<src>.<pin>"`), so an input has at
   most one wire and replacing one is a single set. Every edit is `SetSetting` commands; a
   multi-setting edit (paste, delete, reroute, a comment dragging its nodes, applied text) is
   one transaction. No command variant is added (Ch.7 is frozen; ADR 0026 did the same for
   the domain editors). `Graphs::follow` applies the setting change log to the touched nodes.
3. **Nodes come from `#[forge_api]` registries** (`NodeLibrary`): the editor's built-in
   nodes are real `#[forge_api]` functions (math, logic, physics, material, generator, PCG)
   and the component catalogue's types join them. A node's category decides which graph kinds
   may use it; a mutating item is blueprint-only. `check_connect` uses `forge_reflect::connect`
   (identical types; units of the same dimension, with the conversion scale), refuses cycles,
   and follows reroutes to the type that reaches them.
4. **The IR is a trait, `GraphIr`, with the labelled in-memory `StubIr`** (D-4): compile to
   ops in dependency order, readable Rust text, diagnostics mapped to nodes with the
   contract's `GRAPH-*` codes, and `parse_text` + `apply_text` for the graph ↔ text round
   trip. It builds nothing; gate row `C-graph-ir-backend` is `UNBUILT` until `forge-graph`.
5. **Wall-clock UI gates run at the High priority class**
   (`forge_ui::testing::run_ahead_of_background_work`, read back), and the mirror's undo
   history evicts in O(1) amortised instead of shifting 10,000 entries per transaction.

## Alternatives rejected

- **Wires as their own keyed objects:** would allow two wires into one input and needs a
  consistency pass; the field-of-the-input form makes the invariant structural.
- **One widget painting the minimap too:** every pan would re-record every node's dot
  (the thing `ui_graph_2k_nodes` forbids).
- **lyon path strokes for wires:** ~3 ms per frame for ~200 curves with round joins; the
  polyline strip is ~15x cheaper and looks the same at wire widths.
- **Widening the hierarchy's 2 ms budget, or retrying:** forbidden (W5), and the slowness
  was partly real (the history shift).

## Consequences

- `forge-graph` implements `GraphIr` and replaces `StubIr` in `EditorServices::graph`; the
  panel, the settings format and the `GRAPH-*` codes stay.
- Guards: `C-ui-graph-budget` (`ui_graph_2k_nodes`, control: every node recorded),
  `C-graph-unit-check` and `C-graph-edits-undoable` (`test_graph_editor`, controls: the check
  skipped; edits not grouped), `C-ui-timed-priority` (control: a class that did not take).
