//! The `forge-editor` binary's library half: the program itself ([`app`], which an edition's
//! binary runs with its own [`app::EditorEdition`]), and what the binary adds to the editor that
//! needs a GPU — the viewport's `forge-render` host ([`viewport_gpu`]). Kept in a library so its
//! guard (`tests/test_viewport_gpu.rs`) drives exactly the code the binary runs.

#![forbid(unsafe_code)]

pub mod app;
pub mod first_party;
pub mod gpu_mode;
pub mod licence;
pub mod viewport_gpu;
pub mod wiring;
