//! The regionized scheduler (Ch.5.3) — single-threaded skeleton that already proves
//! disjointness.
//!
//! A tick is: **plan** (group systems into waves), **prove** (no two jobs in a wave can
//! touch the same entity/component in conflicting ways), **run** (every job of every wave,
//! each through a [`RegionView`]), **barrier** (apply every [`Outbox`] in a fixed order).
//!
//! This skeleton runs the jobs of a wave one after another on the calling thread (M0-9: "not
//! yet parallel"). The point of building the proof now is that a wave is *exactly* the set
//! of jobs the parallel scheduler (M4-1) will hand to threads at once: the proof is the
//! permission, and it is already enforced and fuzzed (`tests/test_region_disjointness.rs`)
//! before a second thread exists to break it.

use forge_frames::Tick;

use crate::outbox::Message;
use crate::view::Scope;
use crate::{CoreError, EntityId, Error, Outbox, RegionAccess, RegionId, RegionView, World};

/// A system that runs once per region per tick (Ch.5.3).
pub trait RegionSystem: Send {
    /// Diagnostic name (error breadcrumbs, conflict reports).
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// Declares what it touches. The scheduler proves disjointness before running, and the
    /// [`RegionView`] enforces the declaration while it runs.
    fn access(&self) -> RegionAccess;

    /// Run against one region. Mutate only through `r`; everything that reaches outside the
    /// region goes into `out`.
    fn run(&mut self, r: &mut RegionView<'_>, out: &mut Outbox);
}

/// Index of a system in its [`Scheduler`], in registration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemIndex(pub usize);

/// One system run against one region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Job {
    /// Which system.
    pub system: SystemIndex,
    /// Which region.
    pub region: RegionId,
}

/// Jobs that may run concurrently: pairwise, either different regions (disjoint entity
/// sets) or the same region with non-conflicting access.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wave {
    /// The jobs, in the order the single-threaded skeleton runs them.
    pub jobs: Vec<Job>,
}

/// A tick's waves, in order. Systems in later waves see the writes of earlier ones.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The waves, in order.
    pub waves: Vec<Wave>,
}

/// What one tick did.
#[derive(Debug)]
pub struct TickReport {
    /// The tick that ran (the world's tick before it advanced).
    pub tick: Tick,
    /// The plan that was proven and run.
    pub plan: Plan,
    /// Barrier messages applied.
    pub applied: usize,
    /// Barrier messages rejected, each with its code and the sending region/system.
    pub rejected: Vec<Error>,
}

/// Runs [`RegionSystem`]s over every region of a [`World`], one tick at a time.
#[derive(Default)]
pub struct Scheduler {
    systems: Vec<Box<dyn RegionSystem>>,
}

impl Scheduler {
    /// No systems.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a system. Systems run in registration order wherever their accesses
    /// conflict; non-conflicting neighbours share a wave.
    pub fn add_system(&mut self, s: impl RegionSystem + 'static) -> SystemIndex {
        self.systems.push(Box::new(s));
        SystemIndex(self.systems.len() - 1)
    }

    /// Number of systems.
    #[must_use]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// No systems.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    fn accesses(&self) -> Vec<RegionAccess> {
        self.systems.iter().map(|s| s.access()).collect()
    }

    fn name(&self, i: SystemIndex) -> String {
        self.systems
            .get(i.0)
            .map_or_else(|| format!("#{}", i.0), |s| s.name().to_owned())
    }

    /// Group systems into stages: a run of consecutive systems whose accesses pairwise do not
    /// conflict. Each stage becomes one wave over every region (regions in id order).
    #[must_use]
    pub fn plan(&self, world: &World) -> Plan {
        plan_from(&self.accesses(), world)
    }

    /// Prove a plan safe to run concurrently wave by wave, against the world's current
    /// ownership table and the systems' current declarations:
    ///
    /// 1. every job names a real system and a real region;
    /// 2. no two jobs in one wave on the same region conflict in access — `CORE-0008`;
    /// 3. no entity is held by two regions (so no two different-region jobs in any wave share
    ///    an entity) — `CORE-0007`.
    ///
    /// This read-only form re-derives (3) from scratch (`Regions::check_invariants`,
    /// `O(entities)`); [`Scheduler::tick`] proves it incrementally
    /// (`Regions::prove_disjoint`, `O(entities moved since the last tick)`).
    pub fn verify(&self, plan: &Plan, world: &World) -> Result<(), CoreError> {
        verify_jobs(&self.accesses(), plan, world, |i| self.name(i))?;
        world.regions().check_invariants()
    }

    /// Plan, prove, run and barrier one tick.
    pub fn tick(&mut self, world: &mut World) -> Result<TickReport, Error> {
        let plan = self.plan(world);
        self.tick_with_plan(world, plan)
    }

    /// Prove and run a caller-supplied plan (the scheduler refuses an unsafe one before any
    /// system runs). Declarations are read **once**: the proof and the run-time enforcement
    /// use the same snapshot, so a system cannot declare one thing and be held to another.
    pub fn tick_with_plan(&mut self, world: &mut World, plan: Plan) -> Result<TickReport, Error> {
        let tick = world.tick();
        let accesses = self.accesses();
        // The proof. Region membership cannot change until the barrier, so one proof of entity
        // disjointness covers every wave of the tick.
        verify_jobs(&accesses, &plan, world, |i| self.name(i))
            .and_then(|()| world.regions_mut().prove_disjoint())
            .map_err(|e| Error::from(e).ctx("tick", tick.0))?;

        let mut outboxes: Vec<(RegionId, SystemIndex, Outbox)> = Vec::new();
        for wave in &plan.waves {
            for job in &wave.jobs {
                let (ecs, region, owners) = world.split_for(job.region)?;
                let access = accesses
                    .get(job.system.0)
                    .ok_or(CoreError::UnknownSystem(job.system.0))?;
                let system = self
                    .systems
                    .get_mut(job.system.0)
                    .ok_or(CoreError::UnknownSystem(job.system.0))?;
                let mut view = RegionView::new(ecs, region, owners, Scope::Declared(access));
                let mut out = Outbox::new(job.region);
                system.run(&mut view, &mut out);
                if !out.is_empty() {
                    outboxes.push((job.region, job.system, out));
                }
            }
        }

        // Barrier. Order by (sending region, system), keeping each outbox's send order: the
        // same messages apply in the same order however the waves were packed.
        outboxes.sort_by_key(|(r, s, _)| (*r, *s));
        let mut applied = 0;
        let mut rejected = Vec::new();
        for (from, sys, out) in outboxes {
            for msg in out.msgs {
                match apply(world, from, msg) {
                    Ok(()) => applied += 1,
                    Err(e) => rejected.push(
                        e.ctx("from_region", from.0)
                            .ctx("system", self.name(sys))
                            .ctx("tick", tick.0),
                    ),
                }
            }
        }
        world.advance_tick()?;
        Ok(TickReport {
            tick,
            plan,
            applied,
            rejected,
        })
    }
}

fn plan_from(accesses: &[RegionAccess], world: &World) -> Plan {
    let mut stages: Vec<Vec<SystemIndex>> = Vec::new();
    for (i, a) in accesses.iter().enumerate() {
        let fits = stages.last().is_some_and(|stage| {
            stage
                .iter()
                .all(|j| accesses[j.0].conflict_with(a).is_none())
        });
        match stages.last_mut() {
            Some(stage) if fits => stage.push(SystemIndex(i)),
            _ => stages.push(vec![SystemIndex(i)]),
        }
    }
    let regions: Vec<RegionId> = world.regions().ids().collect();
    Plan {
        waves: stages
            .into_iter()
            .map(|stage| Wave {
                jobs: regions
                    .iter()
                    .flat_map(|&region| stage.iter().map(move |&system| Job { system, region }))
                    .collect(),
            })
            .collect(),
    }
}

/// Proof steps (1) and (2): jobs name real systems and regions; same-region jobs in a wave do
/// not conflict.
fn verify_jobs(
    accesses: &[RegionAccess],
    plan: &Plan,
    world: &World,
    name: impl Fn(SystemIndex) -> String,
) -> Result<(), CoreError> {
    let regions = world.regions();
    for wave in &plan.waves {
        for job in &wave.jobs {
            if job.system.0 >= accesses.len() {
                return Err(CoreError::UnknownSystem(job.system.0));
            }
            regions.get(job.region)?;
        }
    }
    for wave in &plan.waves {
        let mut by_region: Vec<&Job> = wave.jobs.iter().collect();
        by_region.sort_by_key(|j| (j.region, j.system));
        for group in by_region.chunk_by(|a, b| a.region == b.region) {
            for (i, a) in group.iter().enumerate() {
                for b in &group[i + 1..] {
                    if let Some(k) = accesses[a.system.0].conflict_with(&accesses[b.system.0]) {
                        return Err(CoreError::AccessConflict {
                            a: name(a.system),
                            b: name(b.system),
                            component: k.name(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn apply(world: &mut World, from: RegionId, msg: Message) -> Result<(), Error> {
    let owned_by_sender = |world: &World, entity: EntityId| {
        if world.regions().owner_of(entity) == Some(from) {
            Ok(())
        } else {
            Err(CoreError::NotOwned {
                entity,
                region: from,
            })
        }
    };
    match msg {
        Message::Effect { to, f } => {
            let (ecs, region, owners) = world.split_for(to)?;
            let mut view = RegionView::new(ecs, region, owners, Scope::Barrier);
            f(&mut view)
        }
        Message::Transfer { entity, to } => {
            owned_by_sender(world, entity)?;
            Ok(world.transfer(entity, to)?)
        }
        Message::Spawn { to, f } => {
            world.region(to)?;
            let e = EntityId::from_entity(f(world.ecs_mut()));
            Ok(world.assign(e, to)?)
        }
        Message::Despawn { entity } => {
            owned_by_sender(world, entity)?;
            Ok(world.despawn(entity)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Profile;
    use bevy_ecs::component::Component;
    use forge_frames::FrameId;

    #[derive(Component, Debug, PartialEq, Clone, Copy)]
    struct Pos(i64);
    #[derive(Component, Debug, PartialEq, Clone, Copy)]
    struct Vel(i64);
    #[derive(Component, Debug, PartialEq, Clone, Copy)]
    struct Hits(i64);

    /// pos += vel.
    struct Integrate;
    impl RegionSystem for Integrate {
        fn access(&self) -> RegionAccess {
            RegionAccess::new().write::<Pos>().read::<Vel>()
        }
        fn run(&mut self, r: &mut RegionView<'_>, _out: &mut Outbox) {
            let es: Vec<EntityId> = r.entities().collect();
            for e in es {
                if let Ok(v) = r.get::<Vel>(e).copied()
                    && let Ok(mut p) = r.get_mut::<Pos>(e)
                {
                    p.0 += v.0;
                }
            }
        }
    }

    /// Reads Vel only — shares a wave with Integrate? No: Integrate reads Vel too, and
    /// neither writes it, so yes.
    struct CountMovers(usize);
    impl RegionSystem for CountMovers {
        fn access(&self) -> RegionAccess {
            RegionAccess::new().read::<Vel>()
        }
        fn run(&mut self, r: &mut RegionView<'_>, _out: &mut Outbox) {
            self.0 += r
                .entities()
                .filter(|&e| r.has::<Vel>(e) == Ok(true))
                .count();
        }
    }

    /// Writes Vel — conflicts with both of the above.
    struct Damp;
    impl RegionSystem for Damp {
        fn access(&self) -> RegionAccess {
            RegionAccess::new().write::<Vel>()
        }
        fn run(&mut self, r: &mut RegionView<'_>, _out: &mut Outbox) {
            let es: Vec<EntityId> = r.entities().collect();
            for e in es {
                if let Ok(mut v) = r.get_mut::<Vel>(e) {
                    v.0 /= 2;
                }
            }
        }
    }

    /// Hits every entity of region `target`, and hands its first entity to `target`.
    struct Poke {
        target: RegionId,
    }
    impl RegionSystem for Poke {
        fn access(&self) -> RegionAccess {
            RegionAccess::new()
        }
        fn run(&mut self, r: &mut RegionView<'_>, out: &mut Outbox) {
            if r.id() == self.target {
                return;
            }
            out.send(self.target, |t| {
                let es: Vec<EntityId> = t.entities().collect();
                for e in es {
                    let h = t.get::<Hits>(e).map(|h| h.0).unwrap_or(0);
                    t.insert(e, Hits(h + 1))?;
                }
                Ok(())
            });
            if let Some(e) = r.entities().next() {
                out.transfer(e, self.target);
            }
        }
    }

    fn two_regions() -> (World, RegionId, RegionId, Vec<EntityId>) {
        let mut w = World::new();
        let a = w.create_region(FrameId(0), Profile::Surface).expect("a");
        let b = w.create_region(FrameId(0), Profile::Surface).expect("b");
        let mut es = Vec::new();
        for i in 0..3 {
            es.push(w.spawn_in(a, (Pos(0), Vel(i + 1))).expect("spawn"));
            es.push(w.spawn_in(b, (Pos(100), Vel(10))).expect("spawn"));
        }
        (w, a, b, es)
    }

    #[test]
    fn non_conflicting_neighbours_share_a_wave_conflicting_ones_do_not() {
        let (mut w, a, b, _) = two_regions();
        let mut s = Scheduler::new();
        s.add_system(Integrate);
        s.add_system(CountMovers(0));
        s.add_system(Damp);
        let plan = s.plan(&w);
        assert_eq!(plan.waves.len(), 2, "{plan:?}");
        let j = |sys, region| Job {
            system: SystemIndex(sys),
            region,
        };
        assert_eq!(plan.waves[0].jobs, vec![j(0, a), j(1, a), j(0, b), j(1, b)]);
        assert_eq!(plan.waves[1].jobs, vec![j(2, a), j(2, b)]);
        s.verify(&plan, &w).expect("the planner's plan proves");
        let rep = s.tick(&mut w).expect("tick");
        assert_eq!(rep.tick, Tick(0));
        assert_eq!(w.tick(), Tick(1));
    }

    #[test]
    fn systems_see_earlier_waves_and_only_their_region() {
        let (mut w, a, _b, es) = two_regions();
        let mut s = Scheduler::new();
        s.add_system(Integrate);
        s.add_system(Damp);
        s.tick(&mut w).expect("tick");
        s.tick(&mut w).expect("tick");
        // es[0] is in `a` with vel 1: tick 1 pos 1 then vel 0; tick 2 pos 1.
        assert_eq!(w.get::<Pos>(es[0]).ok(), Some(&Pos(1)));
        assert_eq!(w.get::<Vel>(es[0]).ok(), Some(&Vel(0)));
        // es[1] is in `b` with vel 10: 110, vel 5, 115, vel 2.
        assert_eq!(w.get::<Pos>(es[1]).ok(), Some(&Pos(115)));
        assert_eq!(w.regions().owner_of(es[0]), Some(a));
    }

    #[test]
    fn a_conflicting_hand_made_plan_is_refused_before_anything_runs() {
        let (mut w, a, _b, es) = two_regions();
        let mut s = Scheduler::new();
        s.add_system(Integrate);
        s.add_system(Damp);
        let bad = Plan {
            waves: vec![Wave {
                jobs: vec![
                    Job {
                        system: SystemIndex(0),
                        region: a,
                    },
                    Job {
                        system: SystemIndex(1),
                        region: a,
                    },
                ],
            }],
        };
        let err = s.tick_with_plan(&mut w, bad).expect_err("must refuse");
        assert_eq!(err.code().as_str(), "CORE-0008");
        assert_eq!(w.get::<Pos>(es[0]).ok(), Some(&Pos(0)), "nothing ran");
        assert_eq!(w.tick(), Tick(0), "time did not advance");
    }

    #[test]
    fn a_plan_naming_a_missing_system_or_region_is_refused() {
        let (w, a, _b, _) = two_regions();
        let s = Scheduler::new();
        let plan = Plan {
            waves: vec![Wave {
                jobs: vec![Job {
                    system: SystemIndex(0),
                    region: a,
                }],
            }],
        };
        assert_eq!(s.verify(&plan, &w), Err(CoreError::UnknownSystem(0)));
        let mut s = Scheduler::new();
        s.add_system(Damp);
        let plan = Plan {
            waves: vec![Wave {
                jobs: vec![Job {
                    system: SystemIndex(0),
                    region: RegionId(77),
                }],
            }],
        };
        assert_eq!(
            s.verify(&plan, &w),
            Err(CoreError::UnknownRegion(RegionId(77)))
        );
    }

    #[test]
    fn barrier_applies_effects_and_transfers_in_order() {
        let (mut w, a, b, es) = two_regions();
        let mut s = Scheduler::new();
        s.add_system(Poke { target: b });
        let rep = s.tick(&mut w).expect("tick");
        assert!(rep.rejected.is_empty(), "{:?}", rep.rejected);
        assert_eq!(rep.applied, 2);
        // a's first entity moved to b; b's original three were hit once.
        assert_eq!(w.regions().owner_of(es[0]), Some(b));
        for e in [es[1], es[3], es[5]] {
            assert_eq!(w.get::<Hits>(e).ok(), Some(&Hits(1)));
        }
        // The effect ran before the transfer (send order), so the moved entity was not hit.
        assert!(w.get::<Hits>(es[0]).is_err());
        assert_eq!(w.region(a).expect("a").len(), 2);
        w.regions().check_invariants().expect("invariants");
    }

    #[test]
    fn stale_barrier_messages_are_rejected_not_panicked() {
        struct Stale {
            victim: EntityId,
        }
        impl RegionSystem for Stale {
            fn access(&self) -> RegionAccess {
                RegionAccess::new()
            }
            fn run(&mut self, _r: &mut RegionView<'_>, out: &mut Outbox) {
                out.despawn(self.victim); // not ours
                out.transfer(self.victim, RegionId(999));
                out.send(RegionId(999), |_| Ok(()));
                out.spawn_in(RegionId(999), Pos(0));
            }
        }
        let mut w = World::new();
        let a = w.create_region(FrameId(0), Profile::Surface).expect("a");
        let victim = w.spawn(Pos(1));
        let before = w.ecs().entities().len();
        let mut s = Scheduler::new();
        s.add_system(Stale { victim });
        let rep = s.tick(&mut w).expect("tick");
        let codes: Vec<&str> = rep.rejected.iter().map(|e| e.code().as_str()).collect();
        assert_eq!(codes, ["CORE-0005", "CORE-0005", "CORE-0002", "CORE-0002"]);
        assert!(w.contains(victim));
        assert_eq!(
            w.ecs().entities().len(),
            before,
            "rejected spawn spawned nothing"
        );
        let msg = rep.rejected[0].to_string();
        assert!(msg.contains(&format!("from_region={}", a.0)), "{msg}");
    }

    #[test]
    fn spawn_in_at_the_barrier_is_owned() {
        struct Spawner(RegionId);
        impl RegionSystem for Spawner {
            fn access(&self) -> RegionAccess {
                RegionAccess::new()
            }
            fn run(&mut self, _r: &mut RegionView<'_>, out: &mut Outbox) {
                out.spawn_in(self.0, Pos(7));
            }
        }
        let (mut w, a, b, _) = two_regions();
        let mut s = Scheduler::new();
        s.add_system(Spawner(b));
        let rep = s.tick(&mut w).expect("tick");
        assert_eq!(rep.applied, 2, "one spawn from each region");
        assert_eq!(w.region(b).expect("b").len(), 5);
        assert_eq!(w.region(a).expect("a").len(), 3);
        w.regions().check_invariants().expect("invariants");
    }
}
