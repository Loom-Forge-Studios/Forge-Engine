# ADR 0015 — Assets are sidecar-identified, content-addressed, imported through plugins over ProjectStore

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.8 (expanded to FULL here), Ch.33.2 / §33.6, Ch.32.2, Ch.4, I7, I8,
  I16, I17, M0-17, M2-6; WP-U5 consumes the thumbnail service and the asset listing

## Context

Ch.8 was a BRIEF: content-addressed store, stable `AssetId` independent of path, async typed
handles, hot reload, importers as plugins, glTF first, KTX2/Basis textures. It left open where
identity lives, what an artefact is, how hot reload sees changes through a `ProjectStore`
that has no change notification, how import stays a command (I7), and how generated assets
(`SeedPath`, no file) fit. `ProjectStore` is frozen (M0-20); additive methods are allowed.

## Decision

1. **Identity lives in a sidecar** `<source>.meta.ron` (RON, diffable): the `AssetId` (128-bit,
   minted once from the first import path + content hash), the importer, the import settings
   (sorted string map) and the record of the last import. The id travels with the file;
   sub-assets are `id.child(label)` with name-based labels (`mesh:Hull`, index only when a
   name is missing or ambiguous); generated assets are `AssetId::generated(generator, seed)`.
2. **Artefacts are blobs.** Importers emit kind-tagged byte artefacts; the server stores each
   with `blob_put` (BLAKE3, Ch.33.2). The record holds the importer id + version, source hash,
   settings hash, every dependency's hash and every artefact's address; a fresh record means
   the importer does not run, an unchanged address means nothing reloads.
3. **Formats.** Mesh: `FMSH` LE binary with GPU-layout vertex streams (`Float32x3` positions
   etc.) and `u32` indices; texture: a standard KTX2 container (RGBA8 sRGB/UNORM, full mip
   chain, mips box-filtered in linear light with sRGB tables built by `forge_num::det::pow`);
   material and scene: RON. Vertex streams are data: `f32` appears only as a stored format,
   widened to `f64` the moment forge-asset computes with it (bounds, transforms, thumbnails).
4. **Storage only through `ProjectStore` (I17).** A `Vfs` wraps the store; forge-asset has no
   `std::fs` (guard `test_asset_store_only.rs`). `gltf`'s `import` feature is off; buffers and
   images are read through `ImportCx::dependency`, which resolves **exactly** against the
   store listing (M0-17): a wrong-case or backslash reference fails on every platform.
5. **Hot reload by stamps (additive store method).** `ProjectStore::stamps()` returns a cheap
   per-file change token: default = content hash (correct for any backend); `MemoryStore` =
   stored hash; `LocalFs` = size + mtime, **content hash for files modified in the last two
   seconds** (a coarse write clock cannot hide a same-size rewrite) — *amended by ADR 0017:*
   one directory walk, a 100 ms / 2 s racy window with each hash kept, `stamps_of` for the
   poll's own writes, a quiet-poll budget on disk and a watcher plan. `AssetServer::poll`
   diffs stamps, reimports changed sources and dependents (content-checked), re-attaches a
   source renamed without its sidecar by content, applies hand-edited sidecar settings, and
   reloads changed artefacts under live handles (old value visible until the new one lands).
6. **Plugins.** `Importer`, `Exporter`, `AssetType` are the Ch.32.2 seed points; the
   first-party `forge.asset` source plugin registers glTF/image/KTX2 importers, the glTF
   (`.glb`) exporter and the four built-in types through the ordinary loader (I16). A
   middleware importer (chain) post-processes through `ImportCx::artefacts_mut`.
7. **Commands (I7).** `forge.asset.import / remove / rename` record an *intent* in a project
   setting (`asset.import.p<hash>`, RON value); planning never touches the store, so dry run
   equals apply and undo/redo are the bus's. `AssetServer::sync(&Project)` reconciles; a
   vanished/new intent pair linked by `renamed_from` is a rename in either direction.
   `forge.asset.adopt` (not undoable) records assets the database learned of outside the bus.
8. **Handles.** `load::<T>(id)` returns a `Handle<T>` at once, checked against the kind's
   registered `TypeId`; a small worker pool (≤ 4 threads) fetches and decodes; `wait`,
   `ready()` (a `Future`) and `state()` for the three kinds of caller.
9. **Thumbnails** are rendered on the workers from artefacts alone (texture mip box filter,
   isometric flat-shaded software raster for meshes/scenes, lit swatch for materials),
   deterministic, in memory only, refreshed on reload.
10. **forge-asset builds at `opt-level = 2` in dev** (root `Cargo.toml`), like forge-ui
    (ADR 0013).

## Why — the owner's two rules

1. **Better for the user:** renaming a file never breaks a scene; a pull request shows
   "mips: true → false", not a binary diff; a texture referenced as `Rock.PNG` fails on the
   author's Windows machine instead of on the Linux build server; importing, renaming and
   removing assets are undoable and show who did them; a hot reload never flashes a missing
   texture; the asset browser has thumbnails before the renderer exists.
2. **Faster / more efficient engine:** unchanged inputs never re-run an importer (reopen of a
   500-object project runs zero importers); unchanged artefacts never reload; a quiet poll is
   one stamps call (1–5 µs in memory; on a real folder one directory walk, ~3 µs per file —
   ADR 0017); textures are already GPU-layout KTX2 with mips.
   Measured (`test_import_perf.rs`, 12-thread dev box, MemoryStore): 500 objects × 1089
   vertices (28.3 MiB `.bin`, 8 materials, 2 textures) — cold import 51.9 ms release /
   70.5 ms dev, fresh reimport 8.2 ms, load all 500 meshes through handles 4.3 ms, 128 px
   scene thumbnail 69 ms, reopen 9.6 ms; 500 × 25 vertices — cold import 7.4 ms release.

## Alternatives rejected

- *Id from the path* (or a path→id table in `.forge/`): breaks on every rename done outside
  the editor, and `.forge/` is not committed, so identity would not travel with the project.
- *A filesystem watcher (`notify`) inside forge-asset:* bypasses I17 and would not work on
  memory/git/S3 backends; its licence is not on the allow-list either. Stamps work everywhere.
- *Changing the frozen `ProjectStore` signatures:* not needed; `stamps()` is additive with a
  correct default.
- *Import as a direct store write from a command handler:* handlers plan over a read-only
  project; an intent in project state keeps dry-run == apply and makes undo free.
- *Converting vertex data to `f64`:* doubles memory and upload cost for data no forge-asset
  code does arithmetic on; the no-`f32` rule is about simulation math, which stays `f64`.
- *Basis Universal at import now:* a C++ build and a device-dependent target choice; it
  belongs with forge-gpu's format negotiation. Gate row `C-asset-block-compression` (UNBUILT).

## Consequences

- Gate rows BOUND: `C-asset-gltf-roundtrip`, `C-asset-hot-reload`, `C-asset-id-stable`,
  `C-asset-ref-exact-case`, `C-asset-store-only`; UNBUILT: `C-asset-block-compression`,
  `C-asset-usd-fbx`; `C-ui-asset-index` stays UNBUILT with a new reason (the adapter lives in
  WP-U5's `forge-panels-assets`).
- `test_extension_point_replaceable` now covers the three asset points (kits observe by
  importing, exporting and decoding) and requires a kit for every `forge-asset` seed point.
- The M0-17 lint scans `.gltf` files too; import sidecars are covered as RON.
- `tests/liveness/i1_allow.txt` allows two asset-local coordinates (mesh vertex positions,
  a node's offset from its parent) — they get a `FramePos` when an asset is placed.
- Importer authors bump `Importer::version` when output changes; every asset of that
  importer reimports on the next open.
