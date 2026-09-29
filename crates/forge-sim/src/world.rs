//! The simulation world: a `forge_core::World` forked from the edit world, one region per
//! frame, stepped by the regionized scheduler at a fixed step.
//!
//! **Determinism (Ch.3).** Every step is the same arithmetic in the same order on every
//! machine: `f64` throughout, the rotations from `forge_num::det` sine and cosine, entities
//! spawned in key order into regions in frame order, the scheduler's plan proven once at the
//! fork and reused, inputs applied at a step boundary in arrival order. Nothing reads a clock.
//! Two runs that apply the same inputs at the same steps are equal to the bit, which is what
//! a replay's hash check and `test_headless_parity` rely on.
//!
//! **Integration.** Semi-implicit Euler per step: `v += a dt; p += v dt` (in the entity's
//! frame axes), and a spin about the frame's up axis `r = normalise(q(dt) r)`. The fixed
//! step's spin quaternion is computed once per spin, not per step.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy_ecs::component::Component;
use forge_cmd::{EntityKey, Value};
use forge_core::{
    EntityId, Outbox, Plan, Profile, RegionAccess, RegionId, RegionSystem, RegionView, Scheduler,
    World,
};
use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_trace::Tracer;
use serde::{Deserialize, Serialize};

use crate::SimError;
use crate::edit::{
    EditSnapshot, P_ACCELERATION, P_FRAME, P_LOCAL, P_PITCH, P_ROLL, P_SCALE, P_SPIN, P_VELOCITY,
    P_YAW,
};
use crate::phys::{Fork, SimPhysics, Spec};

/// Steps per second.
pub const SIM_RATE_HZ: u32 = 60;
/// The fixed step, seconds.
pub const SIM_DT: f64 = 1.0 / SIM_RATE_HZ as f64;

/// A simulated entity's transform (what the viewport shows in place of the edit world's).
pub type SimTransform = (FramePos, DQuat, f64);

/// Where an entity is.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Placement(pub FramePos);
/// Its axes relative to its frame's.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Orientation(pub DQuat);
/// Uniform scale.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Scale(pub f64);
/// Linear velocity, m/s in the frame's axes.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Velocity(pub DVec3);
/// Linear acceleration, m/s² in the frame's axes.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Acceleration(pub DVec3);
/// Spin about the frame's up axis: degrees per second, and the fixed step's rotation.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Spin {
    pub dps: f64,
    pub step: DQuat,
}

impl Spin {
    #[must_use]
    pub fn new(dps: f64) -> Self {
        Self {
            dps,
            step: spin_quat(dps, SIM_DT),
        }
    }
}

fn spin_quat(dps: f64, dt: f64) -> DQuat {
    DQuat::from_axis_angle(DVec3::Y, (dps * dt).to_radians())
}

/// Yaw (about up), pitch (about right), roll (about forward), degrees — the editor's
/// convention (`q_yaw * q_pitch * q_roll`), with deterministic trigonometry.
#[must_use]
pub fn euler_to_quat(yaw: f64, pitch: f64, roll: f64) -> DQuat {
    DQuat::from_axis_angle(DVec3::Y, yaw.to_radians())
        * DQuat::from_axis_angle(DVec3::X, pitch.to_radians())
        * DQuat::from_axis_angle(DVec3::Z, roll.to_radians())
}

/// What an input does to an entity.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum InputAction {
    /// Set the linear velocity (m/s).
    SetVelocity([f64; 3]),
    /// Add to the linear velocity (m/s): a kick.
    Impulse([f64; 3]),
    /// Set the linear acceleration (m/s²).
    SetAcceleration([f64; 3]),
    /// Set the spin (degrees per second).
    SetSpin(f64),
}

/// An input command to the simulation (a game's input, an automation session's simulation step):
/// applied at the next step boundary and recorded with that step.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimInput {
    pub entity: EntityKey,
    pub action: InputAction,
}

impl SimInput {
    fn finite(&self) -> bool {
        match self.action {
            InputAction::SetVelocity(v)
            | InputAction::Impulse(v)
            | InputAction::SetAcceleration(v) => v.iter().all(|x| x.is_finite()),
            InputAction::SetSpin(s) => s.is_finite(),
        }
    }
}

forge_trace::control_switches! {
    /// Test-only fault switches (W2 positive controls for `test_replay` and
    /// `test_headless_parity`). Never set outside those tests.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SimFaults {
        /// Step with the editor frame's measured time instead of the fixed step (the classic
        /// variable-timestep bug: the result depends on the frame rate).
        pub frame_dt: bool,
        /// Rebuild the moving bodies' transform map from scratch after every batch of steps, as
        /// the WP-13 play core did (an allocation per moving body per frame), instead of
        /// updating it in place (the W2 positive control of `test_play_alloc`).
        pub rebuild_transforms: bool,
    }
}

/// The step length the systems use (f64 bits): the fixed step, unless a fault says otherwise.
type DtCell = Arc<AtomicU64>;

fn dt_of(c: &DtCell) -> f64 {
    f64::from_bits(c.load(Ordering::Relaxed))
}

/// Counts accesses a system could not make (never expected: the systems touch only owned
/// entities and declared components). Non-zero fails the step with `SIM-0008`.
type Faults = Arc<AtomicU64>;

struct Integrate {
    dt: DtCell,
    faults: Faults,
    /// The region's entities, reused across steps (no allocation per step).
    ents: Vec<EntityId>,
}

impl RegionSystem for Integrate {
    fn name(&self) -> &str {
        "sim.integrate"
    }
    fn access(&self) -> RegionAccess {
        RegionAccess::new()
            .read::<Acceleration>()
            .write::<Velocity>()
            .write::<Placement>()
    }
    fn run(&mut self, r: &mut RegionView<'_>, _out: &mut Outbox) {
        let dt = dt_of(&self.dt);
        self.ents.clear();
        self.ents.extend(r.entities());
        for &e in &self.ents {
            if !r.has::<Velocity>(e).unwrap_or(false) {
                continue;
            }
            let a = match r.get::<Acceleration>(e) {
                Ok(a) => Some(a.0),
                Err(_) => None,
            };
            let v = match r.get_mut::<Velocity>(e) {
                Ok(mut v) => {
                    if let Some(a) = a {
                        v.0 += a * dt;
                    }
                    v.0
                }
                Err(_) => {
                    self.faults.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            match r.get_mut::<Placement>(e) {
                Ok(mut p) => p.0.local += v * dt,
                Err(_) => {
                    self.faults.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

struct Turn {
    dt: DtCell,
    faults: Faults,
    ents: Vec<EntityId>,
}

impl RegionSystem for Turn {
    fn name(&self) -> &str {
        "sim.spin"
    }
    fn access(&self) -> RegionAccess {
        RegionAccess::new().read::<Spin>().write::<Orientation>()
    }
    fn run(&mut self, r: &mut RegionView<'_>, _out: &mut Outbox) {
        let dt = dt_of(&self.dt);
        self.ents.clear();
        self.ents.extend(r.entities());
        for &e in &self.ents {
            let Ok(s) = r.get::<Spin>(e).copied() else {
                continue;
            };
            if s.dps == 0.0 {
                continue;
            }
            let q = if dt == SIM_DT {
                s.step
            } else {
                spin_quat(s.dps, dt)
            };
            match r.get_mut::<Orientation>(e) {
                Ok(mut o) => o.0 = (q * o.0).try_normalize().unwrap_or(o.0),
                Err(_) => {
                    self.faults.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// The simulation world (see the module docs).
pub struct SimWorld {
    world: World,
    scheduler: Scheduler,
    plan: Plan,
    keys: BTreeMap<EntityKey, EntityId>,
    dt: DtCell,
    faults: Faults,
    step: u64,
    /// The physics bodies' worlds (`None`: the edit world has no physics body).
    physics: Option<SimPhysics>,
}

impl std::fmt::Debug for SimWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimWorld")
            .field("entities", &self.keys.len())
            .field("step", &self.step)
            .finish_non_exhaustive()
    }
}

fn float(p: &BTreeMap<String, Value>, k: &str, d: f64) -> f64 {
    match p.get(k) {
        Some(Value::Float(v)) => *v,
        Some(Value::Int(v)) => *v as f64,
        _ => d,
    }
}

fn vec3(p: &BTreeMap<String, Value>, k: &str) -> Option<DVec3> {
    match p.get(k) {
        Some(Value::Vec3(v)) => Some(DVec3::new(v[0], v[1], v[2])),
        _ => None,
    }
}

impl SimWorld {
    /// Fork from the edit world: every entity with a transform (`transform.position.local`)
    /// becomes a simulated body, moving if it has motion properties; one with `physics.*`
    /// properties becomes a rigid body on the first-party physics backends (the project's
    /// `physics.backend`, avian3d by default).
    pub fn fork(edit: &EditSnapshot) -> Result<SimWorld, SimError> {
        let backends =
            forge_phys::first_party_backends().map_err(|e| SimError::Physics(e.to_string()))?;
        Self::fork_with(edit, &backends)
    }

    /// [`Self::fork`] with the physics backends a host's plugins registered on
    /// `forge.phys.backend`.
    pub fn fork_with(
        edit: &EditSnapshot,
        backends: &forge_plugin::Registry<forge_phys::PhysicsBackendPoint>,
    ) -> Result<SimWorld, SimError> {
        struct Body {
            key: EntityKey,
            frame: FrameId,
            at: DVec3,
            rot: DQuat,
            scale: f64,
            velocity: Option<DVec3>,
            accel: Option<DVec3>,
            spin: Option<f64>,
            phys: Option<Spec>,
        }
        let mut bodies = Vec::new();
        for e in &edit.entities {
            let p = &e.properties;
            let Some(at) = vec3(p, P_LOCAL) else {
                continue;
            };
            let frame = match p.get(P_FRAME) {
                Some(Value::Int(f)) => {
                    FrameId(u32::try_from(*f).map_err(|_| SimError::BadEditWorld {
                        entity: e.key,
                        detail: format!("{P_FRAME} = {f} is not a frame id"),
                    })?)
                }
                _ => FrameId(0),
            };
            let spin = match p.get(P_SPIN) {
                Some(Value::Float(s)) => Some(*s),
                Some(Value::Int(s)) => Some(*s as f64),
                _ => None,
            };
            let velocity = vec3(p, P_VELOCITY);
            let accel = vec3(p, P_ACCELERATION);
            let scale = float(p, P_SCALE, 1.0);
            let phys = Spec::of(e, scale)?;
            bodies.push(Body {
                key: e.key,
                frame,
                at,
                rot: euler_to_quat(
                    float(p, P_YAW, 0.0),
                    float(p, P_PITCH, 0.0),
                    float(p, P_ROLL, 0.0),
                ),
                scale,
                // Anything that can move gets a velocity, so an acceleration alone moves it.
                velocity: velocity.or(accel.map(|_| DVec3::ZERO)),
                accel,
                spin,
                phys,
            });
        }
        let mut world = World::new();
        let mut regions: BTreeMap<FrameId, RegionId> = BTreeMap::new();
        let mut frames: Vec<FrameId> = bodies.iter().map(|b| b.frame).collect();
        frames.sort_unstable();
        frames.dedup();
        for f in frames {
            let r = world
                .create_region(f, Profile::default())
                .map_err(|e| SimError::Core(e.to_string()))?;
            regions.insert(f, r);
        }
        let mut keys = BTreeMap::new();
        let mut forks = Vec::new();
        for b in bodies {
            let region = regions
                .get(&b.frame)
                .copied()
                .ok_or_else(|| SimError::Core(format!("no region for frame {}", b.frame.0)))?;
            let id = world
                .spawn_in(
                    region,
                    (
                        Placement(FramePos::new(b.frame, b.at)),
                        Orientation(b.rot),
                        Scale(b.scale),
                    ),
                )
                .map_err(|e| SimError::Core(e.to_string()))?;
            let core = |e: forge_core::CoreError| SimError::Core(e.to_string());
            keys.insert(b.key, id);
            if let Some(spec) = b.phys {
                // Physics moves it: no integrator components (the scheduler's systems leave
                // it alone); its motion properties are its starting velocities.
                forks.push(Fork {
                    key: b.key,
                    id,
                    at: FramePos::new(b.frame, b.at),
                    rot: b.rot,
                    spec,
                    velocity: b.velocity.unwrap_or(DVec3::ZERO),
                    spin: b.spin.unwrap_or(0.0),
                    accel: b.accel.unwrap_or(DVec3::ZERO),
                });
                continue;
            }
            if let Some(v) = b.velocity {
                world.insert(id, Velocity(v)).map_err(core)?;
            }
            if let Some(a) = b.accel {
                world.insert(id, Acceleration(a)).map_err(core)?;
            }
            if let Some(s) = b.spin {
                world.insert(id, Spin::new(s)).map_err(core)?;
            }
        }
        let physics = SimPhysics::build(&edit.settings, backends, forks)?;
        let dt: DtCell = Arc::new(AtomicU64::new(SIM_DT.to_bits()));
        let faults: Faults = Arc::new(AtomicU64::new(0));
        let mut scheduler = Scheduler::new();
        scheduler.add_system(Integrate {
            dt: dt.clone(),
            faults: faults.clone(),
            ents: Vec::new(),
        });
        scheduler.add_system(Turn {
            dt: dt.clone(),
            faults: faults.clone(),
            ents: Vec::new(),
        });
        // Regions never change inside a simulation, so the plan is proven here once.
        let plan = scheduler.plan(&world);
        scheduler
            .verify(&plan, &world)
            .map_err(|e| SimError::Core(e.to_string()))?;
        Ok(SimWorld {
            world,
            scheduler,
            plan,
            keys,
            dt,
            faults,
            step: 0,
            physics,
        })
    }

    /// Steps run since the fork.
    #[must_use]
    pub fn step_count(&self) -> u64 {
        self.step
    }

    /// Simulated bodies.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Does the simulation have this entity?
    #[must_use]
    pub fn contains(&self, k: EntityKey) -> bool {
        self.keys.contains_key(&k)
    }

    /// Check an input without applying it.
    pub fn check_input(&self, i: &SimInput) -> Result<(), SimError> {
        if !self.keys.contains_key(&i.entity) {
            return Err(SimError::UnknownEntity(i.entity));
        }
        if !i.finite() {
            return Err(SimError::NonFinite(i.entity));
        }
        Ok(())
    }

    /// Apply an input now (the caller applies inputs at a step boundary).
    pub fn apply_input(&mut self, i: &SimInput) -> Result<(), SimError> {
        self.check_input(i)?;
        if let Some(p) = &mut self.physics
            && p.input(i.entity, i.action)?
        {
            return Ok(());
        }
        let id = self.keys[&i.entity];
        let core = |e: forge_core::CoreError| SimError::Core(e.to_string());
        let v3 = |a: [f64; 3]| DVec3::new(a[0], a[1], a[2]);
        match i.action {
            InputAction::SetVelocity(v) => self.world.insert(id, Velocity(v3(v))).map_err(core),
            InputAction::Impulse(dv) => {
                let v = self.world.get::<Velocity>(id).map_or(DVec3::ZERO, |v| v.0);
                self.world.insert(id, Velocity(v + v3(dv))).map_err(core)
            }
            InputAction::SetAcceleration(a) => {
                if self.world.get::<Velocity>(id).is_err() {
                    self.world.insert(id, Velocity(DVec3::ZERO)).map_err(core)?;
                }
                self.world.insert(id, Acceleration(v3(a))).map_err(core)
            }
            InputAction::SetSpin(s) => self.world.insert(id, Spin::new(s)).map_err(core),
        }
    }

    /// Run one step of `dt` seconds (the fixed step except under a fault).
    pub(crate) fn step_dt(&mut self, dt: f64, tracer: &Tracer) -> Result<(), SimError> {
        let _z = tracer.zone("sim.step");
        self.dt.store(dt.to_bits(), Ordering::Relaxed);
        // The plan proven at the fork runs every step: lent to the scheduler and handed back
        // in its report, never cloned.
        let plan = std::mem::take(&mut self.plan);
        let report = match self.scheduler.tick_with_plan(&mut self.world, plan) {
            Ok(r) => r,
            Err(e) => {
                // The plan went with the failed tick: plan again so the world stays
                // steppable, and report the failure.
                self.plan = self.scheduler.plan(&self.world);
                return Err(SimError::Core(e.to_string()));
            }
        };
        self.plan = report.plan;
        if let Some(e) = report.rejected.first() {
            return Err(SimError::Core(e.to_string()));
        }
        let f = self.faults.swap(0, Ordering::Relaxed);
        if f != 0 {
            return Err(SimError::Core(format!(
                "{f} component access(es) refused inside a step"
            )));
        }
        if let Some(p) = &mut self.physics {
            let _z = tracer.zone("sim.physics");
            p.step(&mut self.world)?;
        }
        self.step += 1;
        Ok(())
    }

    /// Run one fixed step.
    pub fn step(&mut self, tracer: &Tracer) -> Result<(), SimError> {
        self.step_dt(SIM_DT, tracer)
    }

    fn transform(&self, id: EntityId) -> Option<SimTransform> {
        let p = self.world.get::<Placement>(id).ok()?.0;
        let r = self.world.get::<Orientation>(id).ok()?.0;
        let s = self.world.get::<Scale>(id).ok()?.0;
        Some((p, r, s))
    }

    /// Transforms of the bodies that can move (velocity or spin), by key.
    #[must_use]
    pub fn moving_transforms(&self) -> BTreeMap<EntityKey, SimTransform> {
        self.keys
            .iter()
            .filter(|(k, _)| self.is_moving(**k))
            .filter_map(|(k, id)| self.transform(*id).map(|t| (*k, t)))
            .collect()
    }

    /// Whether the body `k` can move (it has a velocity or a spin).
    #[must_use]
    pub fn is_moving(&self, k: EntityKey) -> bool {
        if let Some(m) = self.physics.as_ref().and_then(|p| p.moving(k)) {
            return m;
        }
        self.keys.get(&k).is_some_and(|id| {
            self.world.get::<Velocity>(*id).is_ok() || self.world.get::<Spin>(*id).is_ok()
        })
    }

    /// Refresh, in place, the transforms of the bodies already in `into` (after a batch of
    /// steps): no allocation whatever the number of bodies.
    pub fn refresh_transforms(&self, into: &mut BTreeMap<EntityKey, SimTransform>) {
        for (k, t) in into.iter_mut() {
            if let Some(now) = self.transform_of(*k) {
                *t = now;
            }
        }
    }

    /// Every body's transform.
    #[must_use]
    pub fn transform_of(&self, k: EntityKey) -> Option<SimTransform> {
        self.keys.get(&k).and_then(|id| self.transform(*id))
    }

    /// BLAKE3 over the whole simulation state — the step count and, per body in key order,
    /// every component's bits — as 64 hex digits.
    #[must_use]
    pub fn state_hash(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"forge-sim state v1");
        h.update(&self.step.to_le_bytes());
        h.update(&(self.keys.len() as u64).to_le_bytes());
        let v3 = |h: &mut blake3::Hasher, v: DVec3| {
            for b in v.to_bits() {
                h.update(&b.to_le_bytes());
            }
        };
        for (k, id) in &self.keys {
            h.update(&k.0.to_le_bytes());
            if let Some((p, r, s)) = self.transform(*id) {
                h.update(&p.frame.0.to_le_bytes());
                v3(&mut h, p.local);
                for x in [r.x, r.y, r.z, r.w, s] {
                    h.update(&x.to_bits().to_le_bytes());
                }
            }
            match self.world.get::<Velocity>(*id) {
                Ok(v) => {
                    h.update(&[1]);
                    v3(&mut h, v.0);
                }
                Err(_) => {
                    h.update(&[0]);
                }
            }
            match self.world.get::<Acceleration>(*id) {
                Ok(a) => {
                    h.update(&[1]);
                    v3(&mut h, a.0);
                }
                Err(_) => {
                    h.update(&[0]);
                }
            }
            match self.world.get::<Spin>(*id) {
                Ok(s) => {
                    h.update(&[1]);
                    h.update(&s.dps.to_bits().to_le_bytes());
                }
                Err(_) => {
                    h.update(&[0]);
                }
            }
        }
        // Physics bodies' velocities live in their physics world: hash it too. (A world
        // without physics hashes exactly as before WP-60.)
        if let Some(p) = &self.physics
            && let Err(e) = p.hash_into(&mut h)
        {
            h.update(e.to_string().as_bytes());
        }
        h.finalize().to_hex().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::EditEntity;

    fn body(key: u64, frame: i64, props: Vec<(&str, Value)>) -> EditEntity {
        let mut p: BTreeMap<String, Value> =
            props.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        p.insert(P_FRAME.into(), Value::Int(frame));
        p.entry(P_LOCAL.into())
            .or_insert(Value::Vec3([0.0, 0.0, 0.0]));
        EditEntity {
            key: EntityKey(key),
            name: format!("b{key}"),
            parent: None,
            properties: p,
        }
    }

    #[test]
    fn a_falling_body_integrates_semi_implicitly_and_a_spinner_turns() {
        let edit = EditSnapshot::from_entities([
            body(
                1,
                0,
                vec![
                    (P_VELOCITY, Value::Vec3([1.0, 0.0, 0.0])),
                    (P_ACCELERATION, Value::Vec3([0.0, -9.81, 0.0])),
                ],
            ),
            body(2, 3, vec![(P_SPIN, Value::Float(90.0))]),
            body(3, 0, vec![]),
        ]);
        let mut w = SimWorld::fork(&edit).unwrap();
        assert_eq!(w.len(), 3);
        let t = Tracer::new();
        for _ in 0..60 {
            w.step(&t).unwrap();
        }
        let (p, _, _) = w.transform_of(EntityKey(1)).unwrap();
        assert!((p.local.x - 1.0).abs() < 1e-12, "{p:?}");
        // Semi-implicit Euler over 60 steps: y = -g dt² * (1 + 2 + ... + 60).
        let expect = -9.81 * SIM_DT * SIM_DT * (60.0 * 61.0 / 2.0);
        assert!(
            (p.local.y - expect).abs() < 1e-9,
            "{} vs {expect}",
            p.local.y
        );
        let (p2, r2, _) = w.transform_of(EntityKey(2)).unwrap();
        assert_eq!(p2.frame, FrameId(3));
        // 90 deg/s for one second: a quarter turn about up.
        let x = r2.rotate(DVec3::X);
        assert!(
            (x.z + 1.0).abs() < 1e-9 || (x.z - 1.0).abs() < 1e-9,
            "{x:?}"
        );
        assert_eq!(w.moving_transforms().len(), 2);
        assert_eq!(w.world.regions().len(), 2, "one region per frame");
    }

    #[test]
    fn inputs_are_checked() {
        let edit = EditSnapshot::from_entities([body(1, 0, vec![])]);
        let mut w = SimWorld::fork(&edit).unwrap();
        let bad = SimInput {
            entity: EntityKey(9),
            action: InputAction::SetSpin(1.0),
        };
        assert_eq!(
            w.apply_input(&bad),
            Err(SimError::UnknownEntity(EntityKey(9)))
        );
        let nan = SimInput {
            entity: EntityKey(1),
            action: InputAction::Impulse([f64::NAN, 0.0, 0.0]),
        };
        assert_eq!(w.apply_input(&nan), Err(SimError::NonFinite(EntityKey(1))));
        let kick = SimInput {
            entity: EntityKey(1),
            action: InputAction::Impulse([0.0, 2.0, 0.0]),
        };
        let h0 = w.state_hash();
        w.apply_input(&kick).unwrap();
        assert_ne!(w.state_hash(), h0);
        assert_eq!(w.moving_transforms().len(), 1, "a kicked body moves");
    }

    #[test]
    fn a_negative_frame_is_refused_with_its_entity() {
        let edit = EditSnapshot::from_entities([body(4, -1, vec![])]);
        let e = SimWorld::fork(&edit).unwrap_err();
        assert!(matches!(e, SimError::BadEditWorld { entity, .. } if entity == EntityKey(4)));
    }
}
