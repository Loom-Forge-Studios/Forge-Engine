//! `forge-phys` — 3D physics (Ch.17; M4-3, M7-9 first half), for both editions.
//!
//! * [`types`] — the backend-neutral model: rigid bodies (static, kinematic, dynamic; CCD,
//!   axis locks, damping, gravity scale), colliders (sphere, box, capsule, cylinder, cone,
//!   convex hulls generated from mesh vertices, concave triangle meshes, height fields) with
//!   physics materials (friction, restitution, density, combine rules), collision layers and
//!   triggers, the joint set (fixed, hinge, spring, slider, cone-twist, 6DOF), gravity and
//!   damping zones, and ray / shape / overlap queries.
//! * [`backend`] — the `forge.phys.backend` extension point and [`PhysicsBackend`]. Two
//!   first-party backends, both `f64` and cross-platform deterministic: **avian3d** (E-5, the
//!   default; `avian` feature) and **rapier3d** (`rapier` feature). The project's
//!   `physics.backend` setting picks one; a plugin can add, replace or chain one.
//! * [`character`] — the kinematic character controller ([`Character`]): an upright capsule
//!   that collides and slides, stands on walkable slopes, steps up small steps and snaps to
//!   the ground; shape casts only, so the same on every backend (movement modes: WP-61).
//! * [`world`] — [`PhysicsWorld`]: one region-local world (Ch.17: "avian inside a region
//!   whose origin is frame-local") over a backend, with the deterministic fixed step, render
//!   interpolation, zones, canonical events, batched and async queries and the state hash —
//!   done once above the backends, so switching backend changes the solver's numbers and
//!   nothing else.
//!
//! **Determinism (Ch.3).** `f64` throughout (I1: no `f32` below the renderer); both backends
//! are built with `enhanced-determinism` (portable `libm` math, no SIMD, no parallel solver);
//! bodies, colliders and joints are added to a backend in id order; events and overlaps come
//! out sorted. The same inputs give the same [`PhysicsWorld::state_hash`] on Windows and
//! Linux (`tests/test_phys_determinism.rs` pins it per backend).
//!
//! **Cost when unused.** A world with no zones skips the zone pass; no async query, no queue
//! work; nothing here runs unless a simulation has bodies (the play core creates a world only
//! for an edit world with physics bodies).

#![forbid(unsafe_code)]

#[cfg(feature = "avian")]
mod avian;
pub mod backend;
pub mod character;
pub mod debug;
mod error;
pub mod plugin;
#[cfg(feature = "rapier")]
mod rapier;
pub mod scenes;
pub mod types;
pub mod world;

pub use backend::{
    AVIAN, BackendFactory, FIRST_PARTY, PhysicsBackend, PhysicsBackendPoint, RAPIER,
    first_party_backends,
};
pub use character::{Character, CharacterDesc, MoveResult};
pub use debug::{DebugKind, DebugLine};
pub use error::PhysError;
pub use plugin::PhysPlugin;
pub use types::{
    AxisLocks, AxisMotion, BodyDesc, BodyId, BodyKind, BodyState, ColliderDesc, ColliderId,
    CombineRule, Hit, JointDesc, JointFrames, JointId, JointKind, Layers, Material, Overlap,
    PhysEvent, Query, QueryFilter, QueryResult, Ray, Shape, ShapeCast, ZoneDesc, ZoneEffect,
    ZoneId, ZoneShape,
};
pub use world::{PhysicsSettings, PhysicsWorld, QueryTicket};

forge_trace::control_switches! {
    /// Test-only fault switches (W2 positive controls). Never set outside those tests.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct PhysFaults {
        /// Skip the zone pass (the control of the zone tests: a body in a reversed-gravity
        /// zone falls).
        pub ignore_zones: bool,
        /// Draw every body at its last step's pose (the control of the interpolation test: the
        /// drawn motion stutters at the step rate).
        pub no_interpolation: bool,
        /// Rebuild the moving-body list every step, as a naive world would (the control of
        /// `test_phys_step_alloc`: an allocation per step).
        pub rebuild_moving: bool,
        /// Run the backend's step this many extra times per step (the perf gate's control:
        /// `phys.step.*` must fail).
        pub step_repeats: u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phys_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in PhysError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-phys |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }
}
