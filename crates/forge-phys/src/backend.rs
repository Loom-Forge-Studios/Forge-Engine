//! The `forge.phys.backend` extension point (Ch.32.2's seed list): what a physics backend
//! does, and the registry that picks one per project.
//!
//! A backend is a solver and a collision pipeline behind [`PhysicsBackend`]. It never sees
//! the zones, the interpolation, the event order, the async queue or the state hash: the
//! [`crate::PhysicsWorld`] above it does those once, for every backend, so a project that
//! switches backend keeps every behaviour but the solver's own numbers.
//!
//! The first-party backends are `avian3d` (the default, E-5) and `rapier3d` (feature
//! `rapier`); a plugin adds another, replaces one, or chains one (to log or filter) with the
//! registry's ordinary operations (I16). The project's `physics.backend` setting names the
//! one a simulation uses ([`create`]).

use std::sync::Arc;

use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_plugin::{ExtensionPoint, Order, PluginId, Registry};

use crate::types::{
    BodyDesc, BodyId, BodyState, ColliderDesc, ColliderId, Hit, JointDesc, JointFrames, JointId,
    Overlap, PhysEvent, QueryFilter, Ray, ShapeCast,
};
use crate::{PhysError, PhysicsSettings};

/// The first-party backend ids this build carries, default first.
pub const FIRST_PARTY: &[&str] = &[
    #[cfg(feature = "avian")]
    AVIAN,
    #[cfg(feature = "rapier")]
    RAPIER,
];

/// The avian3d backend's id (the 3D preset's default, `presets/3d/workspace.ron`).
pub const AVIAN: &str = "avian3d";
/// The rapier backend's id.
pub const RAPIER: &str = "rapier3d";

/// The project setting that names a simulation's backend.
pub const SETTING: &str = "physics.backend";

/// A physics backend: one region-local world, in one frame (the world checks frames before
/// calling; a backend uses `FramePos::local`). `Sync`: a query batch reads it from several
/// threads at once. Every method is deterministic: the same calls
/// in the same order give the same bits on every machine (Ch.3).
///
/// Ids are the world's (dense, never reused); a backend maps them to its own handles.
pub trait PhysicsBackend: Send + Sync {
    /// The backend's id on `forge.phys.backend`.
    fn id(&self) -> &str;
    fn set_gravity(&mut self, gravity: DVec3);
    fn add_body(&mut self, id: BodyId, desc: &BodyDesc) -> Result<(), PhysError>;
    /// Remove a body (the world has removed its colliders and joints first).
    fn remove_body(&mut self, id: BodyId) -> Result<(), PhysError>;
    fn add_collider(
        &mut self,
        id: ColliderId,
        body: BodyId,
        desc: &ColliderDesc,
    ) -> Result<(), PhysError>;
    fn remove_collider(&mut self, id: ColliderId) -> Result<(), PhysError>;
    fn add_joint(
        &mut self,
        id: JointId,
        desc: &JointDesc,
        frames: &JointFrames,
    ) -> Result<(), PhysError>;
    fn remove_joint(&mut self, id: JointId) -> Result<(), PhysError>;
    fn state(&self, id: BodyId) -> Result<BodyState, PhysError>;
    /// Teleport a body and set its velocities (wakes it).
    fn set_state(&mut self, id: BodyId, state: &BodyState) -> Result<(), PhysError>;
    /// Set a body's velocities (wakes it).
    fn set_velocity(&mut self, id: BodyId, linear: DVec3, angular: DVec3) -> Result<(), PhysError>;
    /// Apply a linear impulse at the centre of mass and an angular impulse (wakes it).
    fn apply_impulse(&mut self, id: BodyId, linear: DVec3, angular: DVec3)
    -> Result<(), PhysError>;
    /// Move a kinematic body to this pose by the end of the next step (its velocity is what
    /// gets it there, so what it carries moves with it).
    fn set_kinematic_target(
        &mut self,
        id: BodyId,
        target: FramePos,
        rotation: DQuat,
    ) -> Result<(), PhysError>;
    /// A dynamic body's mass (kg); 0 for static and kinematic bodies.
    fn mass(&self, id: BodyId) -> Result<f64, PhysError>;
    fn is_sleeping(&self, id: BodyId) -> Result<bool, PhysError>;
    /// Bring derived state (mass properties) up to date after bodies or colliders changed,
    /// before it is read between steps. A step always does this itself.
    fn sync(&mut self) -> Result<(), PhysError> {
        Ok(())
    }
    /// Advance by `dt` seconds. Contact and trigger begin/end events are appended to `events`
    /// (any order: the world sorts them).
    fn step(&mut self, dt: f64, events: &mut Vec<PhysEvent>) -> Result<(), PhysError>;
    fn cast_ray(&self, ray: &Ray, filter: &QueryFilter) -> Result<Option<Hit>, PhysError>;
    fn cast_shape(&self, cast: &ShapeCast, filter: &QueryFilter) -> Result<Option<Hit>, PhysError>;
    /// Every collider touching `q`, appended to `out` (any order: the world sorts them).
    fn overlap(
        &self,
        q: &Overlap,
        filter: &QueryFilter,
        out: &mut Vec<ColliderId>,
    ) -> Result<(), PhysError>;
    /// Solid contact pairs in touch after the last step (a probe for tests and the profiler).
    fn contact_count(&self) -> usize;
    /// A convex-hull collider's hull edges in its shape's frame, for the debug drawing
    /// ([`crate::debug`]); `None` (the default) draws its vertices' bounds instead.
    fn hull_edges(&self, _id: ColliderId) -> Option<Vec<[DVec3; 2]>> {
        None
    }
}

/// What `forge.phys.backend` holds: a factory for a backend in a frame.
pub type BackendFactory = Arc<
    dyn Fn(FrameId, &PhysicsSettings) -> Result<Box<dyn PhysicsBackend>, PhysError> + Send + Sync,
>;

/// The `PhysicsBackend` point (`forge.phys.backend`). Registry key: the backend id.
pub struct PhysicsBackendPoint;

impl ExtensionPoint for PhysicsBackendPoint {
    type Item = BackendFactory;
    const ID: &'static str = "forge.phys.backend";
    const NAME: &'static str = "PhysicsBackend";
}

/// The first-party factory of `id`, if this build has it.
#[must_use]
pub fn first_party_factory(id: &str) -> Option<BackendFactory> {
    match id {
        #[cfg(feature = "avian")]
        AVIAN => Some(Arc::new(|frame, s| {
            Ok(Box::new(crate::avian::AvianBackend::new(frame, s)?) as Box<dyn PhysicsBackend>)
        })),
        #[cfg(feature = "rapier")]
        RAPIER => Some(Arc::new(|frame, s| {
            Ok(Box::new(crate::rapier::RapierBackend::new(frame, s)?) as Box<dyn PhysicsBackend>)
        })),
        _ => None,
    }
}

/// A registry holding the first-party backends (what [`crate::PhysPlugin`] installs), for a
/// host that runs without the plugin loader (`forge --headless`, tests).
pub fn first_party_backends() -> Result<Registry<PhysicsBackendPoint>, PhysError> {
    let owner =
        PluginId::new(crate::plugin::PLUGIN_ID).map_err(|e| PhysError::Backend(e.to_string()))?;
    let mut reg = Registry::new();
    for id in FIRST_PARTY {
        if let Some(f) = first_party_factory(id) {
            reg.add(owner.clone(), id, f, Order::Last)
                .map_err(|e| PhysError::Backend(e.to_string()))?;
        }
    }
    Ok(reg)
}

/// A copy of `reg`: the same factories (shared), in the same order, under the same owners —
/// what a host hands its simulations from the registry its plugin load filled.
pub fn copy_registry(
    reg: &Registry<PhysicsBackendPoint>,
) -> Result<Registry<PhysicsBackendPoint>, PhysError> {
    let mut out = Registry::new();
    for (k, f) in reg.iter() {
        let owner = match reg.provenance(k) {
            Some(p) => p.owner.clone(),
            None => PluginId::new(crate::plugin::PLUGIN_ID)
                .map_err(|e| PhysError::Backend(e.to_string()))?,
        };
        out.add(owner, k, f.clone(), Order::Last)
            .map_err(|e| PhysError::Backend(e.to_string()))?;
    }
    Ok(out)
}

/// A backend by id from `reg` (the project's `physics.backend`; `None`: the registry's
/// first, which is the default).
pub fn create(
    reg: &Registry<PhysicsBackendPoint>,
    id: Option<&str>,
    frame: FrameId,
    settings: &PhysicsSettings,
) -> Result<Box<dyn PhysicsBackend>, PhysError> {
    let factory = match id {
        Some(id) => reg
            .get(id)
            .ok_or_else(|| PhysError::UnknownBackend(id.to_owned()))?,
        None => reg
            .iter()
            .next()
            .map(|(_, f)| f)
            .ok_or_else(|| PhysError::UnknownBackend("(none registered)".to_owned()))?,
    };
    factory(frame, settings)
}
