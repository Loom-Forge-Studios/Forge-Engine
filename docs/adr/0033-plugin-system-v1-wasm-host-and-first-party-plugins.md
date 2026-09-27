# ADR 0033 — Plugin system v1: a separate WASM host crate, per-call grants, plan-only commands, three subsystems moved out

- **Status:** accepted
- **Date:** 2026-09-22
- **Plan references:** Ch.32 (§32.3, §32.4, §32.7), Ch.8.1, Ch.31, Ch.33.1, E-24, E-27, I7, I16,
  I17, M2-13, M2-14, S10

## Context

WP-15 builds M2-13 (the WASM host, manifests, capability grants, hot reload, I16 bound) and
M2-14 (three first-party subsystems shipped as plugins). The plan fixes the contract (Ch.32);
these choices were left open.

## Decision

1. **The host is its own crate, `forge-wasm`.** The repo tree listed "wasm+source loaders"
   under `forge-plugin`, but every crate links `forge-plugin`; putting wasmtime there would
   make every crate — games' runtime included — compile a JIT (minutes of cold build, a large
   binary) to use none of it. `forge-plugin` gains one additive trait (`HostedPlugin`) and one
   function (`loader::load_hosted`), so WASM plugins still go through the one loader, the one
   manifest check and the one conflict check. *(Rule 2: build and binary cost land only on
   hosts.)*
2. **wasmtime 49, no default features** (`cranelift`, `component-model`, `runtime`, `std`,
   `wat`): Apache-2.0 WITH LLVM-exception, on the I13 list.
3. **One export, `call(point, key, input) -> result<list<u8>, string>`**, not a typed export
   per point. A plugin serves every item its manifest declares through it, and a point's
   crate (or the host) adds an *adapter* to reach a new point without changing the world.
   Typed per-point worlds can come when the graph and generator points exist (M4); the
   byte-level call measured below is the floor either way.
4. **No WASI.** The only imports are `forge:plugin/host` (`log`) and
   `forge:plugin/project-read` (`Fs(ProjectRead)`, through the host's store reader, I17).
   Anything else does not link (`WASM-0003`). Nothing reaches the file system, network, clock
   or environment except through an interface a capability names. *(Rule 1: a plugin the
   user drops in cannot do what its manifest does not say.)*
5. **Capabilities are checked twice, the second time per call on a shared table.** An import
   links only if the manifest requests its capability (the plugin manager shows the request
   before anything runs). Each call through it, and each command plan, checks `SharedGrants`
   — an `Arc<RwLock<Grants>>` handle — at that moment, so a revoke stops the plugin's next
   call. *(Rule 1: revocation is immediate; E-27: one table.)*
6. **WASM commands plan; the bus applies.** A WASM `Command` returns a list of edits
   (`spawn`, `rename`, `set_property`, …; `"$N"` for the plan's own spawns) that the host
   replays onto the command's `DiffBuilder`. The guest never holds project state, so a plugin
   command is undoable, dry-runnable and provenance-tagged by construction (I7, I8), and a
   plan's destructive ops need `Command(Destructive)`. Reads of scene state during a plan are
   not in v1 (the guest gets the arguments; a read-request round trip is a follow-up).
7. **Fuel, not epochs, bounds a call** (default 2·10⁹ units, 256 MiB memory). Epoch
   interruption needs a ticker thread, which would wake an idle editor (D-5). Fuel costs
   throughput inside the guest — it is included in the S10 numbers — and is deterministic.
   A trap discards the instance and the next call starts fresh, so one bad input does not
   disable a plugin.
8. **Hot reload swaps code under the installed items; declarations never change live.**
   Items hold a handle on the plugin's slot; a reload compiles and instantiates the new code
   outside the lock and swaps it in. A failed compile keeps the old code (a half-saved file
   never takes a plugin down). A changed manifest is `WASM-0011`: the host reloads the set
   through the loader, because conflicts were checked against the old declarations. The
   watcher polls two files per plugin from the loop the editor already runs: no thread, no
   read while nothing changed. **Source plugins** reload by an incremental rebuild and
   restart; §32.3's dev-dylib path would need unsafe FFI across Rust's unstable ABI — the
   failure mode §32.3 itself rejects — so it is recorded UNBUILT (`C-source-plugin-hot-reload`).
9. **The three subsystems moved (M2-14)** are the ones the backlog named:
   - **`forge.importers`** — the glTF importer/exporter and PNG/JPEG/KTX2 importers, with their
     `gltf`, `base64` and JPEG dependencies, out of `forge-asset`. `forge-asset` keeps the
     points, the asset types and `texture_from_rgba8` (made the shared, public texture
     builder, so a third-party importer's textures hash like the first-party ones).
     `StorePath::file_name` was added to `forge-store` because the importers needed the file
     name and the only way to it was a `pub(crate)` helper in `forge-asset` — exactly the back
     door I16 exists to find. The importer crate keeps `forge-asset`'s dev-build optimisation
     (ADR 0015).
   - **`forge.presets`** — the built-in presets, as `Preset` items carrying their files; the
     editor builds its presets from the registry, so a third-party preset stands beside them.
   - **`forge.store`** — the `file` and `memory` store-backend providers, out of
     `forge-store`. `LocalFs` and `MemoryStore` stay library types: kernel code (the GPU
     shader watcher, the project host) constructs them as values, which is using a library,
     not a plugin point.
   The editor library's convenience constructors (`builtin_preset`, the in-memory asset
   catalog) load the importer and preset plugins through the ordinary loader: the dependency
   points from host to plugin, never back.
10. **I16 is a public-surface scan plus a loader equivalence.** The scan resolves every path
    a `plugins/*` crate names into a kernel crate against that crate's public surface (not
    just `pub` — also not `#[doc(hidden)]`, not included by `#[path]`/`include!`, no
    privileged feature, no minted `PluginId` or mutable registry). The runtime half loads
    every first-party plugin under its own id and under a third-party id and requires the
    same registry. One reasoned exception: the panels' W2 fault switches (`PanelFaults`).

## Consequences

- S10 early numbers (release, dev box, fuel on): empty call 228 ns; a call with one
  capability-checked host import 500 ns; a 4096-sample generator-node kernel 14.1 µs vs
  4.8 µs native (**2.96×**), bit-identical output. At the top of §32.3's 1.5–3× band: a
  WASM generator node is viable for per-chunk work, and the M4-15 decision point is whether
  fuel stays on for trusted per-sample kernels.
- Calls into one plugin are serialised (one instance per plugin). Parallel generator work
  across threads will want an instance pool; that is an M4 concern with the numbers above.
- `AssetServer::open_memory` takes its extensions explicitly (a server without importers
  was a silent trap once importers left the kernel).
- WASM `Preset` items are read at install; hot-reloading a preset plugin's code does not
  change the registered data until the set reloads (declarative data, like a manifest).
