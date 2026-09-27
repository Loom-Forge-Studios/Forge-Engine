# ADR 0008 — `#[forge_api]`: one parse, four outputs, proven to agree at registration

- **Status:** accepted
- **Date:** 2026-09-20

## Context

Ch.6 asks for one annotation that emits a `bevy_reflect` registration, a blueprint node
descriptor, a JSON Schema fragment and a command variant for mutating items, with a guard
(`test_four_outputs_agree`) that they describe the same arity and types. It leaves open: how
reflection reaches the cross-cutting types that sit *below* forge-reflect, what "registration"
means for a free fn, how items are discovered, what the unit grammar is, how purity is decided,
and which inspector annotations exist (WP-U6 generates the inspector from them).

## Decision

1. **Crates.** `forge-reflect` (runtime: `ForgeType`, `TypeDesc`, `Unit`, descriptors,
   `check_agreement`, `ForgeRegistry`) and `forge-reflect-macros` (the attribute), nested at
   `crates/forge-reflect/macros` — a path dependency inside the workspace is a member without
   touching the root members glob (D-6), and plan-coverage counts it under forge-reflect.
2. **`bevy_reflect =0.19.1`** (same exact pin as `bevy_ecs`, S6), features `std` +
   `reflect_documentation` only.
3. **Output 1 for a fn** is the `TypeInfo` of every by-value parameter and of the success
   return type, obtained as `<T as Typed>::type_info()` from the signature and registered in
   the registry's `TypeRegistry`. No `bevy_reflect` `functions` feature: graphs compile, they
   are never dispatched dynamically (I10), and it would force `Reflect` on `forge_core::Error`.
4. **Separate token lists.** The macro parses once and emits node pins, schema properties and
   command fields as three independently generated lists beside the reflect view, so the
   checker compares four real outputs. `bevy_reflect`'s derive (field names, order, doc
   comments via `reflect_documentation`) is the independent witness for structs and enums.
5. **Registration is explicit and checked**: `ForgeRegistry::register::<T>()` validates the
   annotations and runs `check_agreement`; a drifted item is refused (`REFLECT-0006`) and never
   served. A fn is named by a hidden braced struct with the fn's name (type namespace only, so
   it cannot clash with the fn). No linker-section auto-registration (not in the offline
   registry, and fragile across static linking); the API-surface test (Ch.6 rule 1) will catch
   an item nobody registered.
6. **Purity is inferred** from context parameters — any `&mut` reference parameter: `Mutate`
   (and a command variant); any `&`: `Read`; none: `Pure`. Reference parameters are context,
   never pins.
7. **Units**: one grammar file (`units_core.rs`) compiled into both crates via `#[path]`, so an
   unknown unit is a compile error and the runtime check can never disagree with it. Angle is
   its own dimension. Connecting pins requires identical types (`REFLECT-0007`) and, when both
   carry units, equal dimensions (`REFLECT-0008`), returning the scale factor (`km`→`m` 1000).
8. **Inspector annotations** (the set WP-U6 consumes): `name`, `category`, `units`, `min`,
   `max`, `step`, `range = a..=b`, `read_only`, `hidden`, `widget`, `entity`, tooltip = doc
   comment (`doc = ".."` for fn parameters, which cannot carry doc comments). They surface
   identically in the node pin, the schema (`title`, `description`, `minimum`, `maximum`,
   `readOnly`, `x-forge-*`) and `ForgeRegistry::inspector`. Wrong-kind use (`units` on a
   `bool`, a range on text) is refused at registration (`REFLECT-0002`).
9. **Supported shapes now**: free fns, structs with named fields, enums with unit or
   named-field variants. Generics, methods, tuple structs and tuple variants are compile errors
   with a message (follow-ups), because each needs a schema shape decision of its own.
10. **Crates that do not depend on `bevy_reflect`** can use `#[forge_api]`: the macro
    glob-imports `forge_reflect::__private::reexport::*` beside the derive (bevy's derive names
    a bare `bevy_reflect::` path). Glob imports of one item never conflict, so any number of
    annotated items share a module. Proven by the guard, which lives in such a crate.

## Why — the owner's two rules

2. **Faster engine**: all of it is built once at registration — descriptors are plain data,
   there is no per-call reflection, no dynamic fn dispatch and no linker-section scanning at
   start-up; pin connection is an 8-byte dimension compare.

## Alternatives rejected

- **`bevy_reflect` `functions` + `FunctionRegistry` for fns** — dynamic calls are what I10
  forbids at runtime, and every return type would need `Reflect` (including `Error`).
- **Derive `Reflect` with fields for `FramePos`/`FrameVel`** — needs `Reflect` on forge-num's
  `DVec3` (forge-num depends on nothing) or remote reflection (`unsafe`).
- **`inventory` auto-registration** — not available offline, fragile under static linking, and
  hides the registration order.
- **One shared pin list for all outputs** — the guard would be comparing a list with itself.
- **`multipleOf` for `step`** — JSON validators reject `0.30000000000000004` against
  `multipleOf: 0.1`; `step` is an editor hint (`x-forge-step`), not a validity rule.

## Consequences

- Gate row `C-four-outputs-agree` BOUND: `tests/liveness/test_four_outputs_agree.rs`, controls
  `positive_control_drifted_descriptors_fail` (16 drift kinds, each must be refused) and
  `positive_control_mutate_drift_build_fails` (the `mutate-drift` macro drops the last node
  pin/field/variant; the guard fails with `REFLECT-0006`).
- Every crate that lands `#[forge_api]` items adds them to the guard's `engine_registry()`.
- forge-frames and forge-core now depend on `bevy_reflect` (compile-time only cost; NOTICES
  regenerated, cargo-deny green).
- Follow-ups: generics, methods, tuple structs/variants, `Vec`/`Option` pins, and the Ch.6
  API-surface test ("a public item that is not `#[forge_api]` is an internal detail").
