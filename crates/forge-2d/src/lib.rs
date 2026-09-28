//! `forge-2d` — the 2D pipeline (Ch.35; DoD M4-11; Spike S13).
//!
//! **True 2D, not 3D with an orthographic camera** (O-7): its own render path, its own
//! physics solver, positions as `DVec2` under the `FramePos` discipline
//! ([`forge_frames::FramePos2`], a depth-1 frame tree). It touches the core, the GPU device
//! layer and the asset system and **nothing a 3D world needs** — a 2D project links no crate
//! that declares itself 3D-only (`not-in-2d`: `forge-sky` and the rest;
//! `test_2d_tax_is_zero`, I15 / Ch.31 §31.5).
//!
//! Exactly the Ch.35 §35.2 list (S13: anything else is post-1.0):
//!
//! | §35.2 item | Module |
//! |---|---|
//! | sprite batcher with atlas packing | [`sprite`], [`atlas`] |
//! | tilemaps with autotiling and layers | [`tilemap`] |
//! | spline-based geometry (Sprite-Shape equivalent) | [`spline`] |
//! | 2D lights, shadows and normal maps | [`light`], and the renderer |
//! | 2D physics (a 2D solver, not a 3D one with a frozen axis) | [`physics`] |
//! | `Skeleton2D` cutout rigging | [`skeleton`] |
//! | sprite sheets and frame animation | [`anim`] |
//! | Aseprite import | [`aseprite`], [`plugin`] (`Importer("aseprite")`) |
//! | 2D navigation | [`nav`] |
//! | parallax layers | [`parallax`] |
//! | pixel-perfect camera with integer scaling | [`camera`] |
//! | 2D particles | [`particles`] |
//! | 2D-specific inspector and panel layout in the 2D preset | [`components`] (the inspector's 2D components), `presets/2d/` |
//!
//! Everything above the renderer is `f64` and deterministic (Ch.3): physics, particles,
//! skeletons, autotiling, packing and navigation join the I2 corpus. The renderer
//! (`render` feature, on by default) narrows to `f32` in exactly one module,
//! `render/last_mile.rs` (`tests/liveness/test_no_f32_below_render.rs`).

#![forbid(unsafe_code)]

pub mod anim;
pub mod aseprite;
pub mod atlas;
pub mod camera;
pub mod components;
mod error;
pub mod light;
pub mod math;
pub mod nav;
pub mod parallax;
pub mod particles;
pub mod physics;
pub mod plugin;
#[cfg(feature = "render")]
pub mod render;
pub mod scenes;
pub mod skeleton;
pub mod spline;
pub mod sprite;
pub mod tilemap;

pub use error::Error2d;
pub use forge_frames::{FrameId, FramePos2};
pub use forge_num::DVec2;
pub use plugin::Plugin2d;

forge_trace::control_switches! {
    /// W2 fault switches for the 2D guards' positive controls (test builds only).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Faults2d {
        /// Physics: forget last step's impulses (no warm starting): stacks sag and jitter.
        pub no_warm_start: bool,
        /// Physics: ignore contacts until the shapes overlap (no speculative contacts): a
        /// fast body tunnels through a thin wall.
        pub no_speculative: bool,
        /// Physics: test every pair of colliders instead of sorting and sweeping.
        pub brute_force_pairs: bool,
        /// Physics: solve contacts in reverse order on odd steps (order-dependent results).
        pub reverse_contact_order: bool,
        /// Physics: solve contacts in a `std` `HashSet`'s iteration order, freshly seeded
        /// every step (the classic nondeterminism: two runs of the same world disagree).
        pub hash_ordered_contacts: bool,
        /// Physics: run the velocity iterations this many extra times per step (a solver
        /// several times slower, for the perf gate's `phys2d.step` control).
        pub solver_repeats: u32,
        /// Renderer: evaluate every light this many extra times per pixel.
        pub light_repeats: u32,
        /// Renderer: never merge sprites into batches (one draw call per sprite).
        pub no_batching: bool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_2d_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in Error2d::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-2d |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }
}
