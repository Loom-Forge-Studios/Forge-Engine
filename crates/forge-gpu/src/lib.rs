//! `forge-gpu` — the GPU device layer (Ch.9). `wgpu` is the only graphics API surface.
//!
//! * **Adapter pool** ([`AdapterPool`]): every adapter the driver lists is probed, the O-8
//!   floor (Vulkan 1.2 / Direct3D 12 FL 12_0, compute required) is applied with a
//!   user-facing message when nothing qualifies, one physical GPU exposed on several
//!   backends is counted once, and each selected adapter gets its own device and queue. The
//!   pool is a `Vec` even when its length is 1.
//! * **Transfers** ([`transfer`]): upload, readback, and host-staged cross-adapter copies —
//!   the committed path of spike S1 (ADR 0020).
//! * **Render graph** ([`RenderGraph`]): declared reads/writes, computed order, dead-pass
//!   culling, transient aliasing, a barrier plan, and a runtime check that a pass touches
//!   only what it declared. [`verify_plan`] re-checks a plan independently.
//! * **Shaders** ([`shader`]): WGSL only, validated by naga, hot reloaded from the project
//!   store with the last good module kept in service on error.
//!
//! `unsafe` is allowed in this crate (Ch.1.5) and confined to [`probe`], where the two
//! numbers the floor is stated in are read from wgpu-hal.
//!
//! ```no_run
//! use forge_gpu::{AdapterPool, PoolOptions};
//! let pool = AdapterPool::new(&PoolOptions::default())?;
//! println!("primary: {}", pool.primary().label());
//! for row in pool.report() {
//!     println!("  {} -> {:?}", row.facts.label(), row.verdict);
//! }
//! # Ok::<(), forge_gpu::GpuError>(())
//! ```

mod error;
mod exec;
pub mod floor;
mod graph;
mod pool;
pub mod probe;
pub mod shader;
pub mod transfer;

pub use error::GpuError;
pub use exec::{
    ExecStats, GpuTimer, Imports, PassContext, PassFn, PassTiming, RetainedPassFn, TransientPool,
    read_timestamps,
};
pub use floor::{AdapterFacts, ApiLevel, FloorMiss, check_floor};
pub use graph::{
    Access, Barrier, BufferDesc, CompileOptions, CompiledGraph, GraphPlan, Handle, PassBuilder,
    PassInfo, RenderGraph, ResourceId, ResourceInfo, ResourceKind, SlotInfo, TextureDesc, Use,
    verify_plan,
};
pub use pool::{
    AdapterPool, AdapterReport, AdapterVerdict, Choice, GpuDevice, GpuMode, PoolOptions,
    SoftwarePolicy, plan_selection, plan_selection_in,
};
pub use shader::{ShaderEvent, ShaderId, ShaderLibrary};
/// The wgpu this crate is built on; dependents name it through here so there is one copy.
pub use wgpu;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gpu_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in GpuError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-gpu |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }

    #[test]
    fn vk_version_unpacks() {
        // VK_MAKE_API_VERSION(0, 1, 3, 280)
        let packed = (1u32 << 22) | (3 << 12) | 280;
        assert_eq!(probe::vk_version(packed), (1, 3));
        assert_eq!(probe::vk_version((1 << 22) | (1 << 12)), (1, 1));
    }
}
