//! `forge-render` — the renderer (Ch.10; DoD M1-3, M1-7).
//!
//! * **The last mile** ([`last_mile`], Ch.2.4). Scenes arrive as [`FramePos`]itions in `f64`.
//!   Every frame the camera is resolved in its frame, every instance and light is resolved
//!   into the camera's frame through the `FrameResolver` seam, made camera-relative in `f64`,
//!   rotated into view axes, and only then narrowed to `f32`. The GPU never sees anything but
//!   camera-relative `f32`; no view matrix carries a translation. This crate is the only one
//!   allowed an `f32` position, and inside it `last_mile.rs` is the only file that narrows
//!   (`tests/liveness/test_no_f32_below_render.rs`).
//! * **Depth passes** ([`depth`], Ch.2.4): reversed-Z, drawn back to front into one HDR
//!   target with independent depth clears; the resolver's `ViewDepth` names them (one pass
//!   from 1 cm to 1e4 km for a single-frame world).
//! * **Clustered forward+** ([`cluster`]), **PBR metal-rough** (GGX / height-correlated Smith
//!   / Schlick), **directional + point + spot lights**, **cascaded sun shadows** ([`shadow`];
//!   the documented fallback for virtual shadow maps, ADR 0023).
//! * **Per-pass timing.** Every pass is a `forge_gpu` render-graph pass; with
//!   [`RenderOptions::timing`] each frame reports CPU recording time and GPU time per pass.
//!
//! [`FramePos`]: forge_frames::FramePos

#![forbid(unsafe_code)]

pub mod atmosphere;
mod camera;
pub mod cluster;
pub mod depth;
mod error;
pub mod last_mile;
mod prepare;
mod renderer;
mod scene;
pub mod shadow;

pub use atmosphere::{
    AtmosphereReport, AtmosphereSettings, CentreOffset, GpuAtmosphere, ProbeQuery, ProbeResult,
    TABLE_FORMAT,
};
pub use camera::{Camera, quat_from_basis};
pub use cluster::{ClusterGrid, ClusterScratch, ClusterView, Clusters, LightSphere};
pub use depth::{DepthSetup, DepthSpan, MAX_DEPTH_PASSES, ViewDepth};
pub use error::RenderError;
pub use prepare::{Draw, DrawOrder, PassDraws, PrepareStats, PreparedFrame, SpriteRecord};
pub use renderer::{DEPTH_FORMAT, FrameReport, HDR_FORMAT, RenderOptions, Renderer};
pub use scene::{
    DirectionalLight, Instance, Material, MaterialId, MeshData, MeshId, MeshPoolId, PunctualLight,
    Scene, SceneAtmosphere, SkyPoint, SkyPoints, SkyShine, SkySprite, Spot, Vertex,
};
pub use shadow::{Cascade, CascadeSettings};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_render_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in RenderError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-render |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }
}
