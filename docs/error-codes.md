# Error codes — the one allocator (Ch.1.2, W8)

Every error variant in every crate carries a stable `ErrorCode`, allocated **here and only
here**. `cargo xtask allocators` (part of `just verify`) fails on a duplicate code, an
out-of-order code, or a code whose prefix is not registered.

Rules:

- A code is `<PREFIX>-NNNN`. Each crate owns exactly one prefix, so two lanes working in
  different crates can never collide on a number.
- Codes are allocated **append-only within their prefix** and are **never reused**. A code
  whose variant is removed stays in the table, marked `retired`, so a log line from an old
  build still resolves to its meaning.
- A code is greppable: the string appears verbatim in the variant's docs and its `Display`.
- Register a new crate's prefix in the table below before allocating its first code.

## Prefixes

| Prefix | Crate |
|---|---|
| NUM | forge-num |
| FRAMES | forge-frames |
| SEED | forge-seed |
| CORE | forge-core |
| REFLECT | forge-reflect |
| CMD | forge-cmd |
| ASSET | forge-asset |
| GPU | forge-gpu |
| RENDER | forge-render |
| SKY | forge-sky |
| PCG | forge-pcg |
| PHYS | forge-phys |
| NAV | forge-nav |
| ANIM | forge-anim |
| AUDIO | forge-audio |
| UI | forge-ui |
| EDITOR | forge-editor |
| SCRIPT | forge-script |
| GRAPH | forge-graph |
| NET | forge-net |
| JOBS | forge-jobs |
| HAL | forge-hal |
| PLAY | forge-play |
| TRACE | forge-trace |
| SIM | forge-sim |
| PLUGIN | forge-plugin |
| STORE | forge-store |
| PROJECT | forge-project |
| REMOTE | forge-remote |
| TWOD | forge-2d |
| IDENTITY | forge-identity |
| COLLAB | forge-collab |
| LICENCE | forge-licence |
| RUNTIME | forge-runtime |
| CLI | forge-cli |
| BAKE | forge-bake |
| FARM | forge-farm |
| SERVER | forge-server |
| WASM | forge-wasm |
| SCENE | forge-scene |
| INPUT | forge-input |

## Codes

| Code | Crate | Variant | Meaning |
|---|---|---|---|
| FRAMES-0001 | forge-frames | `FrameError::UnknownFrame` | the frame is not one the resolver knows (the world frame, or a frame a resolver added) |
| FRAMES-0002 | forge-frames | `FrameError::Disconnected` | no path connects the two frames |
| SEED-0001 | forge-seed | `SeedPathError::MissingRoot` | a seed path does not start with the root tag |
| SEED-0002 | forge-seed | `SeedPathError::BadSegment` | a seed path segment is not `tag:index` |
| UI-0001 | forge-ui | UiError::Theme | a theme file is malformed or lacks a token |
| UI-0002 | forge-ui | UiError::UnknownWidget | a widget id is not in the tree (removed or never added) |
| UI-0003 | forge-ui | UiError::DuplicateKey | two children of one parent share a key |
| UI-0004 | forge-ui | UiError::Layout | the layout engine rejected an operation |
| UI-0005 | forge-ui | UiError::Image | an image could not be added to the image atlas |
| UI-0006 | forge-ui | UiError::Gpu | the GPU renderer failed (adapter, device, surface, readback) |
| UI-0007 | forge-ui | UiError::Platform | the window system or event loop failed |
| UI-0008 | forge-ui | UiError::Dock | a dock layout is invalid: a layout file does not parse or is from a newer editor, or an operation names a panel, area or split that is not there |
| EDITOR-0001 | forge-editor | `EditorError::KeyConflict` | a chord would be bound twice in overlapping key contexts, or one binding is a prefix of another (both actions are named) |
| EDITOR-0002 | forge-editor | `EditorError::BadChord` | a chord's text does not parse (`Ctrl+Shift+P`, `Ctrl+K Ctrl+S`) |
| EDITOR-0003 | forge-editor | `EditorError::UnknownAction` | no action (or palette entry) has this id |
| EDITOR-0004 | forge-editor | `EditorError::ActionDisabled` | the action exists but is not available in the current context |
| EDITOR-0005 | forge-editor | `EditorError::Preset` | a workspace preset manifest or one of its files is missing or invalid |
| EDITOR-0006 | forge-editor | `EditorError::Io` | reading or writing a per-user configuration file failed |
| EDITOR-0007 | forge-editor | `EditorError::Layout` | a layout file is invalid (carries the dock's UI-0008 message) |
| EDITOR-0008 | forge-editor | `EditorError::KeymapFile` | a keymap file does not parse or is from a newer editor |
| EDITOR-0009 | forge-editor | `EditorError::Settings` | a settings type cannot be rendered by the settings window (not a reflected struct, or a field type it has no editor for) |
| EDITOR-0010 | forge-editor | `EditorError::Plugin` | the editor's plugins did not load (a manifest, a registration conflict, a capability); carries the plugin kernel's message |
| EDITOR-0011 | forge-editor | `EditorError::Play` | the play core refused a play control, an input or a replay (the `SIM-*` error follows) |
| EDITOR-0012 | forge-editor | `EditorError::Remote` | a remote pairing or exposure change was refused: a wrong or expired pairing code, LAN exposure without its explicit confirmation, an unknown device |
| EDITOR-0013 | forge-editor | `EditorError::PluginIndex` | the plugin index or the downloaded-plugin cache failed: no such plugin or version, a fetch or cache write failed, a cached plugin does not load |
| EDITOR-0014 | forge-editor | `EditorError::Refused` | a panel refused an input before sending any command (an empty or duplicate name, a value out of range, a step the current state does not allow) |
| EDITOR-0015 | forge-editor | `EditorError::Unexplained` | a problem was shown without a well-formed code, a description or a next step (an editor defect; it is shown anyway and counted) |
| CORE-0001 | forge-core | `CoreError::UnknownEntity` | the entity does not exist in this world |
| CORE-0002 | forge-core | `CoreError::UnknownRegion` | no region has this id (never created, merged away, or removed) |
| CORE-0003 | forge-core | `CoreError::AlreadyOwned` | the entity is owned by another region; ownership moves only by transfer |
| CORE-0004 | forge-core | `CoreError::FrameMismatch` | two regions in different frames cannot merge (Ch.5.3) |
| CORE-0005 | forge-core | `CoreError::NotOwned` | the region does not own the entity it tried to touch, split off, transfer or despawn |
| CORE-0006 | forge-core | `CoreError::UndeclaredAccess` | a region system touched a component its `RegionAccess` does not declare |
| CORE-0007 | forge-core | `CoreError::Overlap` | disjointness proof failed: one entity is held by two regions |
| CORE-0008 | forge-core | `CoreError::AccessConflict` | disjointness proof failed: two same-region jobs in one wave conflict on a component |
| CORE-0009 | forge-core | `CoreError::RegionsExhausted` | every `RegionId` has been used |
| CORE-0010 | forge-core | `CoreError::MissingComponent` | the entity does not have the requested component |
| CORE-0011 | forge-core | `CoreError::UnknownSystem` | a plan names a system index the scheduler does not have |
| CORE-0012 | forge-core | `CoreError::TickOverflow` | canonical time would overflow `i64` ticks |
| CORE-0013 | forge-core | `error::MALFORMED_LOWER_CODE` (no variant) | a crate below forge-core reported a code that is not `PREFIX-NNNN` (ruled out by `allocators`; a fallback, never a panic) |
| REFLECT-0001 | forge-reflect | `ReflectError::InvalidUnit` | a `units = "..."` annotation does not parse (also a compile error in `#[forge_api]`) |
| REFLECT-0002 | forge-reflect | `ReflectError::MetaOnWrongKind` | an annotation is on a pin of the wrong kind (`units` on a `bool`, `range` on a `String`, `entity` on an `f64`) |
| REFLECT-0003 | forge-reflect | `ReflectError::BadRange` | `min > max`, a non-positive `step`, or a non-finite bound |
| REFLECT-0004 | forge-reflect | `ReflectError::DuplicateItem` | two different `#[forge_api]` items claim the same path |
| REFLECT-0005 | forge-reflect | `ReflectError::UnknownItem` | no `#[forge_api]` item is registered under the path |
| REFLECT-0006 | forge-reflect | `ReflectError::Drift` | an item's four outputs (reflect, node, schema, command) disagree |
| REFLECT-0007 | forge-reflect | `ReflectError::IncompatibleTypes` | two pins of different types cannot be connected |
| REFLECT-0008 | forge-reflect | `ReflectError::IncompatibleUnits` | two pins whose units have different dimensions cannot be connected (`N·s` into `m/s`) |
| CMD-0001 | forge-cmd | `CmdError::UnknownEntity` | no project entity has this key (never spawned, or despawned) |
| CMD-0002 | forge-cmd | `CmdError::Cycle` | the reparent would make an entity its own ancestor |
| CMD-0003 | forge-cmd | `CmdError::BadPath` | a property path or setting key is not `ident(.ident)*` |
| CMD-0004 | forge-cmd | `CmdError::NonFinite` | a value contains NaN or an infinity |
| CMD-0005 | forge-cmd | `CmdError::UnknownTxn` | no transaction has this id on this bus |
| CMD-0006 | forge-cmd | `CmdError::TxnState` | the transaction is in the wrong state (command into a committed txn, undo of an undone one, redo of a committed one) |
| CMD-0007 | forge-cmd | `CmdError::BadName` | an entity name is empty, longer than 256 characters, or has a control character |
| CMD-0008 | forge-cmd | `CmdError::Conflict` | a diff's `before` no longer matches the project (undo of an overwritten txn); nothing was applied |
| CMD-0009 | forge-cmd | `CmdError::Panicked` | the command panicked; the bus caught it and applied nothing |
| CMD-0010 | forge-cmd | `CmdError::UnknownHandler` | `Invoke` names a target with no registered handler |
| CMD-0011 | forge-cmd | `CmdError::BadArgs` | `Invoke` arguments are not valid for the target |
| CMD-0012 | forge-cmd | `CmdError::IssuerMismatch` | a command's issuer differs from its transaction's issuer |
| CMD-0013 | forge-cmd | `CmdError::DuplicateCommand` | an envelope with this command id was already applied, or is older than the replay window (a stale resend) |
| CMD-0014 | forge-cmd | `CmdError::PolicyRefused` | refused by policy: human-only command from another issuer, a forbidden undo/redo, or a reserved setting (`security.*`, `plugins.*`) changed by a command that is not its writer |
| CMD-0015 | forge-cmd | `CmdError::Poisoned` | an earlier unrecoverable failure; the bus refuses every command |
| CMD-0016 | forge-cmd | `CmdError::DuplicateHandler` | a handler with this target is already registered |
| PLUGIN-0001 | forge-plugin | `PluginError::BadPluginId` | a plugin id is not lowercase dot-separated segments (`com.example.rivers`) |
| PLUGIN-0002 | forge-plugin | `PluginError::BadItemKey` | an item key is empty, over 128 bytes, or has a character outside `A-Z a-z 0-9 _ - . :` |
| PLUGIN-0003 | forge-plugin | `PluginError::DuplicateItem` | two providers claim the same item (both are named) |
| PLUGIN-0004 | forge-plugin | `PluginError::UnknownItem` | no item has this key at this extension point |
| PLUGIN-0005 | forge-plugin | `PluginError::WrongPoint` | an item id names a different extension point than the registry's |
| PLUGIN-0006 | forge-plugin | `PluginError::Conflict` | two plugins modify one item incompatibly (both replace it, or replace vs remove, or remove vs chain); both are named |
| PLUGIN-0007 | forge-plugin | `PluginError::Manifest` | a plugin manifest does not parse or is invalid |
| PLUGIN-0008 | forge-plugin | `PluginError::EngineMismatch` | a plugin's `engine` requirement does not match this kernel's version |
| PLUGIN-0009 | forge-plugin | `PluginError::DuplicatePlugin` | two loaded plugins share an id |
| PLUGIN-0010 | forge-plugin | `PluginError::UnknownPoint` | a manifest or install names an extension point this engine does not define |
| PLUGIN-0011 | forge-plugin | `PluginError::Undeclared` | a plugin's install did something its manifest does not declare |
| PLUGIN-0012 | forge-plugin | `PluginError::PointIdTaken` | two different item types claim one extension point id or manifest name |
| PLUGIN-0013 | forge-plugin | `PluginError::Panicked` | plugin code panicked during install or inside a `chain` wrapper (caught) |
| PLUGIN-0014 | forge-plugin | `PluginError::WasmHostUnbuilt` | a WASM plugin cannot install: the load was given no WASM host (`forge-wasm` runs WASM plugins) |
| PLUGIN-0015 | forge-plugin | `PluginError::CapabilityDenied` | a principal used a capability it was not granted (or a default policy named a never-default one) |
| PLUGIN-0016 | forge-plugin | `PluginError::Hosted` | a hosted (sandboxed WASM) plugin could not install: its host refused it (the host's `WASM-*` error follows) |
| STORE-0001 | forge-store | `StoreError::BadPath` | a path is not a portable project path (empty/dot segments, reserved characters or Windows device names, trailing dot/space, over-long, under `.forge/`) |
| STORE-0002 | forge-store | `StoreError::NotFound` | no working file, blob, revision or lock has this name |
| STORE-0003 | forge-store | `StoreError::Io` | the backend's I/O failed |
| STORE-0004 | forge-store | `StoreError::Corrupt` | stored bytes do not match their content address, or a record does not parse |
| STORE-0005 | forge-store | `StoreError::Locked` | the path is locked by another identity |
| STORE-0006 | forge-store | `StoreError::NotLockOwner` | only the lock's owner can release it |
| STORE-0007 | forge-store | `StoreError::CaseCollision` | a path differs from an existing file or directory only by letter case |
| STORE-0008 | forge-store | `StoreError::UnknownBackend` | a store URL names a scheme no plugin registered |
| STORE-0009 | forge-store | `StoreError::BadRevId` | a revision id is not 64 lowercase hex digits |
| STORE-0010 | forge-store | `StoreError::Remote` | a remote (a Git remote over HTTP(S) or a path) could not be reached or answered what its protocol does not allow |
| STORE-0011 | forge-store | `StoreError::RemoteMoved` | a push found the remote moved since it was read; nothing was changed on it (pull, then push) |
| STORE-0012 | forge-store | `StoreError::Unauthorized` | the remote refused this identity (sign in, or the token lacks access) |
| CLI-0001 | forge-cli | `CliError::Usage` | the `forge` command line is not understood (unknown command or flag, bad value, a non-empty `--dir`) |
| CLI-0002 | forge-cli | `CliError::Step` | an engine step of `forge spine` failed; the engine error (with its own code) follows |
| CLI-0003 | forge-cli | `CliError::Check` | a `forge spine` self-check failed: the engine ran but produced a wrong result (undo did not restore, reload differs, backends disagree) |
| ASSET-0001 | forge-asset | `AssetError::BadReference` | a reference inside an asset is not a portable project path (backslashes, absolute, a scheme, `..` above the root) |
| ASSET-0002 | forge-asset | `AssetError::CaseMismatch` | a reference resolves only by ignoring letter case (M0-17: it would load on Windows and fail on Linux) |
| ASSET-0003 | forge-asset | `AssetError::MissingReference` | a reference inside an asset names no file |
| ASSET-0004 | forge-asset | `AssetError::NoImporter` | no registered importer handles the file |
| ASSET-0005 | forge-asset | `AssetError::Import` | an importer rejected its input (malformed or unsupported file, or it panicked) |
| ASSET-0006 | forge-asset | `AssetError::UnknownAsset` | no asset has this id or path |
| ASSET-0007 | forge-asset | `AssetError::WrongType` | a typed handle asked for a type the asset's kind does not decode to |
| ASSET-0008 | forge-asset | `AssetError::UnknownKind` | no `AssetType` is registered for the kind |
| ASSET-0009 | forge-asset | `AssetError::Decode` | an artefact's bytes do not decode as its kind |
| ASSET-0010 | forge-asset | `AssetError::Store` | the project store failed (its `STORE-*` code follows) |
| ASSET-0011 | forge-asset | `AssetError::Sidecar` | an import sidecar (`*.meta.ron`) does not parse |
| ASSET-0012 | forge-asset | `AssetError::AlreadyExists` | a rename or import target already exists |
| ASSET-0013 | forge-asset | `AssetError::Timeout` | a load did not finish within the caller's timeout |
| ASSET-0014 | forge-asset | `AssetError::Generator` | a generated asset's generator is unknown or failed |
| ASSET-0015 | forge-asset | `AssetError::Export` | an exporter failed or none handles the asset's kind |
| GPU-0001 | forge-gpu | `GpuError::NoAdapter` | the graphics API reported no adapter at all (no driver, or no software rasteriser) |
| GPU-0002 | forge-gpu | `GpuError::BelowFloor` | adapters exist but none meets the O-8 floor (Vulkan 1.2 / D3D12 FL 12_0, compute); the message is user-facing |
| GPU-0003 | forge-gpu | `GpuError::RequestDevice` | an adapter refused to create a device |
| GPU-0004 | forge-gpu | `GpuError::Shader` | a shader did not parse or validate as WGSL (file, line and column in the message) |
| GPU-0005 | forge-gpu | `GpuError::ShaderIo` | a shader file could not be read (missing, deleted, not UTF-8) |
| GPU-0006 | forge-gpu | `GpuError::Graph` | a render graph is invalid (read before write, stale handle, cycle, undeclared access, unbound import) |
| GPU-0007 | forge-gpu | `GpuError::Transfer` | a transfer or readback failed (buffer could not be mapped) |
| GPU-0008 | forge-gpu | `GpuError::NoPresentableAdapter` | no adapter in the pool can present to the window surface |
| GPU-0009 | forge-gpu | `GpuError::UnknownShader` | a shader handle names nothing in the library |
| GPU-0010 | forge-gpu | `GpuError::Validation` | wgpu reported a validation error for an operation (caught by an error scope) |
| RENDER-0001 | forge-render | `RenderError::Frame` | a position, light or the camera could not be resolved into the camera frame (the `FRAMES-*` error follows) |
| RENDER-0002 | forge-render | `RenderError::Gpu` | the GPU layer failed (the `GPU-*` error follows) |
| RENDER-0003 | forge-render | `RenderError::InvalidScene` | the scene or camera is invalid: non-finite value, zero scale, unknown mesh or material, empty mesh, bad field of view |
| RENDER-0004 | forge-render | `RenderError::Target` | the output target does not match the renderer (size or format), or a frame was prepared for another size |
| TWOD-0001 | forge-2d | `Error2d::Invalid` | a 2D input is invalid: a non-finite value, a zero size, an unknown body, texture or terrain id, a non-convex polygon, a skeleton whose parent comes after its child, a self-crossing shape to fill |
| TWOD-0002 | forge-2d | `Error2d::Frame` | a 2D position is in a frame other than the 2D world's (a 2D world is depth 1: one root frame, Ch.35 §35.3) |
| TWOD-0003 | forge-2d | `Error2d::Aseprite` | an Aseprite file could not be read: truncated, bad magic, an unsupported colour depth or cel type, a canvas over the size cap |
| TWOD-0004 | forge-2d | `Error2d::AtlasFull` | an image is larger than an atlas page (with its padding) |
| TWOD-0005 | forge-2d | `Error2d::Gpu` | the GPU layer failed under the 2D renderer (the `GPU-*` error follows) |
| SKY-0001 | forge-sky | `SkyError::InvalidAtmosphere` | an atmosphere description is invalid (non-finite or non-positive radius, gravity, pressure or temperature; bad mole fractions; aerosol, ozone, albedo or sun size out of range) |
| TRACE-0001 | forge-trace | `TraceError::Io` | a trace file (Perfetto) could not be written |
| TRACE-0002 | forge-trace | `TraceError::BadBudgets` | a budgets file (`tests/perf/budgets.ron` format) does not parse |
| TRACE-0003 | forge-trace | `TraceError::TracyNotBuilt` | the Tracy sink was asked for in a build without forge-trace's `tracy` feature |
| SIM-0001 | forge-sim | `SimError::BadEditWorld` | an edit-world entity cannot be simulated as it stands (its transform frame is not a frame id) |
| SIM-0002 | forge-sim | `SimError::UnknownEntity` | an input names an entity the simulation does not have |
| SIM-0003 | forge-sim | `SimError::NonFinite` | an input carries a NaN or infinite value |
| SIM-0004 | forge-sim | `SimError::NotPlaying` | an input arrived with no simulation running |
| SIM-0005 | forge-sim | `SimError::BadRecording` | a replay file does not parse, or is another format version or step rate |
| SIM-0006 | forge-sim | `SimError::SceneMismatch` | a replay was asked to run against an edit world other than the one it was recorded from |
| SIM-0007 | forge-sim | `SimError::Diverged` | a replay diverged from its recording (the first differing event and its step follow) |
| SIM-0008 | forge-sim | `SimError::Core` | the scheduler refused a simulation tick (the `CORE-*` error follows) |
| SIM-0009 | forge-sim | `SimError::Io` | a replay file could not be read or written |
| SIM-0010 | forge-sim | `SimError::Physics` | the simulation's physics failed: no backend for the project's `physics.backend`, or a physics step failed (the `PHYS-*` error follows) |
| PROJECT-0001 | forge-project | `ProjectError::NotOpen` | a lifecycle operation (save, push, pull, build) needs an open project and none is open |
| PROJECT-0002 | forge-project | `ProjectError::Exists` | a new project was asked for where a Forge project already is |
| PROJECT-0003 | forge-project | `ProjectError::NotAProject` | the location holds no Forge project (no `forge-project.ron`) |
| PROJECT-0004 | forge-project | `ProjectError::BadFiles` | the project's files do not read (bad RON, a newer format, a duplicate entity id) |
| PROJECT-0005 | forge-project | `ProjectError::NoRemote` | push or pull with no remote linked |
| PROJECT-0006 | forge-project | `ProjectError::UnsupportedRemote` | the remote's kind is not supported (SSH Git URLs: use HTTPS) or its store backend is not built yet (S3, SQL: UNBUILT) |
| PROJECT-0007 | forge-project | `ProjectError::Diverged` | the histories do not line up for a fast-forward (a push to a remote that is ahead, a pull into a project that is ahead) |
| PROJECT-0008 | forge-project | `ProjectError::Unsaved` | unsaved changes would be overwritten (a pull, or an open or create over a changed project) |
| PROJECT-0009 | forge-project | `ProjectError::Store` | the project store failed (its `STORE-*` code follows) |
| PROJECT-0010 | forge-project | `ProjectError::Build` | building or exporting failed |
| PROJECT-0011 | forge-project | `ProjectError::SignIn` | signing in to a Git host (GitHub's device flow) failed, was declined, expired, or is not available (no OAuth app configured) |
| PROJECT-0012 | forge-project | `ProjectError::Busy` | another project transfer (push, pull, clone, sign-in, repository creation) is still running off the core's lock; try again when it finishes |
| PROJECT-0013 | forge-project | `ProjectError::Interrupted` | a project transfer stopped before it finished (it panicked, or its thread could not start) |
| PROJECT-0014 | forge-project | `ProjectError::NotPermitted` | the team role (or the licence) of whoever asked does not allow the operation (Ch.37 §37.6; a lapsed Team licence refuses only collaboration commands, E-57) |
| PROJECT-0015 | forge-project | `ProjectError::Team` | a team operation failed: no such team, member or invite, a wrong or used join code, an invite for another account, or it would leave the team without an Owner |
| PROJECT-0016 | forge-project | `ProjectError::BaselineMoved` | the team baseline moved since the sandbox's base: pull (Live does it on its own), then publish |
| PROJECT-0017 | forge-project | `ProjectError::Claimed` | the subtree is claimed by another teammate (Ch.37 §37.4) and is read-only for everyone else |
| PROJECT-0018 | forge-project | `ProjectError::Unresolved` | a pull stopped at conflicts nobody has resolved yet; the Conflicts panel shows each with both sides |
| LICENCE-0001 | forge-licence | (no variant; posted by the premium editor) | the premium editor has no valid, non-lapsed entitlement, so its premium features are off and it runs as the base editor — the project and building/shipping are unaffected (E-57) |
| WASM-0001 | forge-wasm | `WasmError::Compile` | a WASM plugin's code is not a valid WebAssembly component (or WAT text) |
| WASM-0002 | forge-wasm | `WasmError::UndeclaredImport` | a component imports a host interface whose capability its manifest does not request |
| WASM-0003 | forge-wasm | `WasmError::UnknownImport` | a component imports something the plugin host does not provide (WASI, sockets, clocks: no ambient authority) |
| WASM-0004 | forge-wasm | `WasmError::MissingExport` | a component does not export `call` with the `forge:plugin` signature |
| WASM-0005 | forge-wasm | `WasmError::Trap` | the guest trapped (a bug, its fuel budget, its memory limit); it is re-instantiated for the next call |
| WASM-0006 | forge-wasm | `WasmError::Guest` | the guest returned an error for a call |
| WASM-0007 | forge-wasm | `WasmError::NoAdapter` | a WASM plugin declares an operation on a point this host has no adapter for (or not for that operation) |
| WASM-0008 | forge-wasm | `WasmError::Io` | a plugin file could not be read |
| WASM-0009 | forge-wasm | `WasmError::NotWasm` | a manifest given to the WASM host says `kind: Source` |
| WASM-0010 | forge-wasm | `WasmError::BadOutput` | the guest's output for an item does not decode as that point expects |
| WASM-0011 | forge-wasm | `WasmError::ManifestChanged` | a hot reload found the manifest's declarations changed; the plugin set must reload through the loader |
| WASM-0012 | forge-wasm | `WasmError::Plugin` | a plugin-layer error (the `PLUGIN-*` code follows) |
| GRAPH-0001 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::UNKNOWN_OP` | a graph node names an op no `#[forge_api]` item or built-in provides |
| GRAPH-0002 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::UNCONNECTED` | an input has no wire and no literal, and its type has no default (an entity, a struct) |
| GRAPH-0003 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::TYPE` | a wire or literal of the wrong type (a `bool` into an `f64`) |
| GRAPH-0004 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::UNIT` | a wire between units of different dimensions (`N·s` into `m/s`) |
| GRAPH-0005 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::CYCLE` | nodes that feed themselves through a cycle of wires |
| GRAPH-0006 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::NOT_IN_KIND` | a node this kind of graph may not use (a command in a pure material, generator or PCG graph) |
| GRAPH-0007 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::DANGLING` | a wire from a node or pin that does not exist |
| GRAPH-0008 | forge-graph (the Ch.24 IR; `forge_graph::compile`, through the editor's `ForgeGraphIr` since WP-23) | `Diagnostic` `codes::TEXT` | graph text (the graph ↔ text view) that does not parse |
| REMOTE-0001 | forge-remote | `RemoteError::Transport` | the split editor's network or QUIC connection failed (bind, connect, a stream) |
| REMOTE-0002 | forge-remote | `RemoteError::Tls` | TLS or certificate set-up failed, or the peer's certificate is not the pinned one |
| REMOTE-0003 | forge-remote | `RemoteError::Protocol` | a split-editor frame did not decode, was too large, or broke the protocol |
| REMOTE-0004 | forge-remote | `RemoteError::Pairing` | pairing failed: no code on show, a wrong or expired code, a person denied it, or it timed out |
| REMOTE-0005 | forge-remote | `RemoteError::Refused` | the host refused the session: the device is not paired, or may not read the project |
| REMOTE-0006 | forge-remote | `RemoteError::Config` | the device's identity or its paired-host list could not be read or written |
| REMOTE-0007 | forge-remote | `RemoteError::Timeout` | waited too long for the host |
| SCENE-0001 | forge-scene | `SceneError::UnknownScene` | no scene in the project has this id |
| SCENE-0002 | forge-scene | `SceneError::Cycle` | instancing (or moving an instance) here would make a scene contain itself; the message names the loop |
| SCENE-0003 | forge-scene | `SceneError::FromBase` | the node comes from a base scene, so it cannot be deleted or moved in an instance (edit the base scene, or Make Local first) |
| SCENE-0004 | forge-scene | `SceneError::InUse` | the scene still has instances or derived scenes, so it cannot be deleted or moved out of the scene library |
| SCENE-0005 | forge-scene | `SceneError::NotLinked` | the entity is not an instance of a scene or part of one (revert and Make Local need one) |
| SCENE-0006 | forge-scene | `SceneError::Reserved` | `scene.*` properties are scene-composition bookkeeping; only the `forge.scene.*` commands write them |
| SCENE-0007 | forge-scene | `SceneError::BadId` | a scene id is malformed (1-64 of a-z, 0-9, _) or already used |
| SCENE-0008 | forge-scene | `SceneError::NotInstanceRoot` | Make Local works on an instance's root, not on a node inside it |
| INPUT-0001 | forge-input | `InputError::BadOverrides` | a player's binding-overrides file does not parse, or was written by a newer build |
| INPUT-0002 | forge-input | `InputError::Io` | a binding-overrides file could not be read or written |
| INPUT-0003 | forge-input | `InputError::BadProfile` | a profile name is not 1-64 of a-z, A-Z, 0-9, `_`, `-` |
| INPUT-0004 | forge-input | `InputError::NoPlayer` | there is no such local player |
| INPUT-0005 | forge-input | `InputError::Backend` | a device backend (gilrs) could not start; the reason follows |
| PHYS-0001 | forge-phys | `PhysError::Invalid` | a physics input is invalid: a non-finite number, a zero or negative size, a degenerate shape (coplanar hull, mesh without triangles, height grid under 2x2), a joint limit with min > max, an unknown body, collider, joint or zone |
| PHYS-0002 | forge-phys | `PhysError::Frame` | a position is in a frame other than the physics world's (a physics world is region-local, Ch.17) |
| PHYS-0003 | forge-phys | `PhysError::UnknownBackend` | the project's `physics.backend` names a backend no plugin registered on `forge.phys.backend`; the message lists the ones this build has |
| PHYS-0004 | forge-phys | `PhysError::Unsupported` | the backend has no such feature (avian3d: a 6DOF axis combination none of its joints expresses); the message names the backend that has it |
| PHYS-0005 | forge-phys | `PhysError::Backend` | the physics backend failed inside a step, a query or its own bookkeeping (its message follows) |
