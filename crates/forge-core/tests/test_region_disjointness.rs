//! `test_region_disjointness` (Ch.5.4): no two concurrently-running regions hold overlapping
//! entity sets. The merge/split sequence is fuzzed.
//!
//! "Concurrently running" is a **wave**: the set of jobs the scheduler proves safe to run at
//! once (the skeleton runs them one after another; M4-1 will hand them to threads). The
//! property is checked three ways after every fuzzed step:
//!
//! 1. the ownership table re-derived from scratch (`Regions::check_invariants`) is sound;
//! 2. every tick is accepted by the scheduler's own proof;
//! 3. independently of both, instrumented systems record which entities each job actually
//!    touched, and for every wave the touched sets of different regions are disjoint and each
//!    job touched only entities its region owned when the tick began.
//!
//! Positive control (W2): `positive_control_mutate_disjointness_build_fails` rebuilds this
//! file with `--features mutate-disjointness` — `split` then leaves the moved entities in the
//! source region too — and asserts that the property test FAILS there, and that the
//! scheduler's own proof refuses to run the overlapping regions.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_core::bevy_ecs::component::Component;
use forge_core::{
    CoreError, EntityId, Outbox, Profile, RegionAccess, RegionId, RegionSystem, RegionView,
    Scheduler, SystemIndex, World,
};
use forge_frames::FrameId;
use proptest::prelude::*;

const VIOLATION: &str = "DISJOINTNESS VIOLATED";

#[derive(Component, Clone, Copy)]
struct Pos(i64);
#[derive(Component, Clone, Copy)]
struct Vel(i64);

#[derive(Debug)]
struct Touch {
    system: SystemIndex,
    region: RegionId,
    entities: Vec<EntityId>,
    errors: Vec<CoreError>,
}

type Log = Arc<Mutex<Vec<Touch>>>;

#[derive(Clone, Copy)]
enum Kind {
    WritePos,
    ReadVel,
    WriteVel,
    /// Touches its first entity (read Pos) and hands it to the next region at the barrier.
    Migrate,
}

struct Probe {
    idx: SystemIndex,
    kind: Kind,
    log: Log,
    /// Region ids alive at tick start, for Migrate's target choice.
    regions: Arc<Mutex<Vec<RegionId>>>,
}

impl RegionSystem for Probe {
    fn access(&self) -> RegionAccess {
        match self.kind {
            Kind::WritePos => RegionAccess::new().write::<Pos>(),
            Kind::ReadVel => RegionAccess::new().read::<Vel>(),
            Kind::WriteVel => RegionAccess::new().write::<Vel>(),
            Kind::Migrate => RegionAccess::new().read::<Pos>(),
        }
    }

    fn run(&mut self, r: &mut RegionView<'_>, out: &mut Outbox) {
        let owned: Vec<EntityId> = r.entities().collect();
        let mut touched = Vec::new();
        let mut errors = Vec::new();
        let mut note = |res: Result<(), CoreError>, e: EntityId| match res {
            Ok(()) => touched.push(e),
            Err(err) => errors.push(err),
        };
        match self.kind {
            Kind::WritePos => {
                for e in owned {
                    note(r.get_mut::<Pos>(e).map(|mut p| p.0 += 1), e);
                }
            }
            Kind::ReadVel => {
                for e in owned {
                    note(r.get::<Vel>(e).map(|_| ()), e);
                }
            }
            Kind::WriteVel => {
                for e in owned {
                    note(r.get_mut::<Vel>(e).map(|mut v| v.0 = -v.0), e);
                }
            }
            Kind::Migrate => {
                if let Some(&e) = owned.first() {
                    note(r.get::<Pos>(e).map(|_| ()), e);
                    let regions = self.regions.lock().expect("lock");
                    if let Some(&to) = regions.iter().find(|&&x| x > r.id()) {
                        out.transfer(e, to);
                    }
                }
            }
        }
        self.log.lock().expect("lock").push(Touch {
            system: self.idx,
            region: r.id(),
            entities: touched,
            errors,
        });
    }
}

#[derive(Clone, Debug)]
enum Op {
    NewRegion { frame: u8 },
    Spawn { r: u8, n: u8 },
    SpawnLoose,
    Assign { e: u16, r: u8 },
    Transfer { e: u16, r: u8 },
    Release { e: u16 },
    Despawn { e: u16 },
    Merge { a: u8, b: u8 },
    Split { r: u8, mask: u32 },
    RemoveRegion { r: u8 },
    Tick,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => (0u8..2).prop_map(|frame| Op::NewRegion { frame }),
        3 => (any::<u8>(), 1u8..6).prop_map(|(r, n)| Op::Spawn { r, n }),
        1 => Just(Op::SpawnLoose),
        1 => (any::<u16>(), any::<u8>()).prop_map(|(e, r)| Op::Assign { e, r }),
        2 => (any::<u16>(), any::<u8>()).prop_map(|(e, r)| Op::Transfer { e, r }),
        1 => any::<u16>().prop_map(|e| Op::Release { e }),
        1 => any::<u16>().prop_map(|e| Op::Despawn { e }),
        3 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Op::Merge { a, b }),
        3 => (any::<u8>(), any::<u32>()).prop_map(|(r, mask)| Op::Split { r, mask }),
        1 => any::<u8>().prop_map(|r| Op::RemoveRegion { r }),
        4 => Just(Op::Tick),
    ]
}

struct Harness {
    world: World,
    sched: Scheduler,
    log: Log,
    regions: Arc<Mutex<Vec<RegionId>>>,
    live: Vec<EntityId>,
}

impl Harness {
    fn new() -> Self {
        let log: Log = Arc::default();
        let regions: Arc<Mutex<Vec<RegionId>>> = Arc::default();
        let mut sched = Scheduler::new();
        for (i, kind) in [Kind::WritePos, Kind::ReadVel, Kind::WriteVel, Kind::Migrate]
            .into_iter()
            .enumerate()
        {
            sched.add_system(Probe {
                idx: SystemIndex(i),
                kind,
                log: log.clone(),
                regions: regions.clone(),
            });
        }
        let mut world = World::new();
        // Start with two regions in frame 0 and one in frame 1, so merges can fail on frames.
        for f in [0, 0, 1] {
            world
                .create_region(FrameId(f), Profile::Surface)
                .expect("region");
        }
        Self {
            world,
            sched,
            log,
            regions,
            live: Vec::new(),
        }
    }

    fn region(&self, sel: u8) -> Option<RegionId> {
        let ids: Vec<RegionId> = self.world.regions().ids().collect();
        (!ids.is_empty()).then(|| ids[usize::from(sel) % ids.len()])
    }

    fn entity(&self, sel: u16) -> Option<EntityId> {
        (!self.live.is_empty()).then(|| self.live[usize::from(sel) % self.live.len()])
    }

    /// Apply one op. Failures of the op itself (a frame mismatch, an entity already owned)
    /// are legitimate outcomes; what matters is that the table stays sound either way.
    fn apply(&mut self, op: &Op) -> Result<(), TestCaseError> {
        match *op {
            Op::NewRegion { frame } => {
                self.world
                    .create_region(FrameId(u32::from(frame)), Profile::Surface)
                    .expect("region ids do not run out in a test");
            }
            Op::Spawn { r, n } => {
                if let Some(r) = self.region(r) {
                    for i in 0..n {
                        let e = self
                            .world
                            .spawn_in(r, (Pos(0), Vel(i64::from(i))))
                            .expect("spawn into a live region");
                        self.live.push(e);
                    }
                }
            }
            Op::SpawnLoose => {
                let e = self.world.spawn((Pos(0), Vel(1)));
                self.live.push(e);
            }
            Op::Assign { e, r } => {
                if let (Some(e), Some(r)) = (self.entity(e), self.region(r)) {
                    let _ = self.world.assign(e, r);
                }
            }
            Op::Transfer { e, r } => {
                if let (Some(e), Some(r)) = (self.entity(e), self.region(r)) {
                    self.world
                        .transfer(e, r)
                        .expect("transfer to a live region");
                }
            }
            Op::Release { e } => {
                if let Some(e) = self.entity(e) {
                    self.world.release(e);
                }
            }
            Op::Despawn { e } => {
                if let Some(e) = self.entity(e) {
                    self.world.despawn(e).expect("despawn a live entity");
                    self.live.retain(|&x| x != e);
                }
            }
            Op::Merge { a, b } => {
                if let (Some(a), Some(b)) = (self.region(a), self.region(b)) {
                    match self.world.merge_regions(a, b) {
                        Ok(_) | Err(CoreError::FrameMismatch { .. }) => {}
                        Err(other) => prop_assert!(false, "unexpected merge error {other}"),
                    }
                }
            }
            Op::Split { r, mask } => {
                if let Some(r) = self.region(r) {
                    let pick: Vec<EntityId> = self
                        .world
                        .region(r)
                        .expect("live")
                        .entities()
                        .enumerate()
                        .filter(|(i, _)| mask & (1 << (i % 32)) != 0)
                        .map(|(_, e)| e)
                        .collect();
                    self.world
                        .split_region(r, &pick)
                        .expect("split owned entities");
                }
            }
            Op::RemoveRegion { r } => {
                if let Some(r) = self.region(r) {
                    self.world.remove_region(r).expect("remove a live region");
                }
            }
            Op::Tick => self.tick()?,
        }
        if let Err(e) = self.world.regions().check_invariants() {
            prop_assert!(
                false,
                "{VIOLATION}: ownership table unsound after {op:?}: {e}"
            );
        }
        Ok(())
    }

    fn tick(&mut self) -> Result<(), TestCaseError> {
        let owned_at_start: BTreeMap<RegionId, BTreeSet<EntityId>> = self
            .world
            .regions()
            .iter()
            .map(|r| (r.id(), r.entities().collect()))
            .collect();
        *self.regions.lock().expect("lock") = owned_at_start.keys().copied().collect();
        self.log.lock().expect("lock").clear();

        let report = self.sched.tick(&mut self.world).map_err(|e| {
            TestCaseError::fail(format!(
                "{VIOLATION}: the scheduler refused its own plan: {e}"
            ))
        })?;
        prop_assert!(
            report.rejected.is_empty(),
            "barrier rejected messages: {:?}",
            report.rejected
        );

        let log = self.log.lock().expect("lock");
        // Every (system, region) job ran exactly once — the skeleton skips nobody.
        let mut jobs: Vec<(SystemIndex, RegionId)> =
            log.iter().map(|t| (t.system, t.region)).collect();
        jobs.sort();
        let expected: Vec<(SystemIndex, RegionId)> = owned_at_start
            .keys()
            .flat_map(|&r| (0..self.sched.len()).map(move |s| (SystemIndex(s), r)))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        prop_assert_eq!(jobs, expected);

        for t in log.iter() {
            prop_assert!(t.errors.is_empty(), "view refused a probe: {:?}", t.errors);
            let owned = &owned_at_start[&t.region];
            for e in &t.entities {
                prop_assert!(
                    owned.contains(e),
                    "{VIOLATION}: region {:?} touched {e:?}, which it did not own",
                    t.region
                );
            }
        }
        for (w, wave) in report.plan.waves.iter().enumerate() {
            let in_wave: Vec<&Touch> = log
                .iter()
                .filter(|t| {
                    wave.jobs
                        .iter()
                        .any(|j| j.system == t.system && j.region == t.region)
                })
                .collect();
            let mut holder: BTreeMap<EntityId, RegionId> = BTreeMap::new();
            for t in &in_wave {
                for &e in &t.entities {
                    if let Some(&other) = holder.get(&e) {
                        prop_assert!(
                            other == t.region,
                            "{VIOLATION}: wave {w}: regions {other:?} and {:?} both touched {e:?}",
                            t.region
                        );
                    }
                    holder.insert(e, t.region);
                }
            }
        }
        Ok(())
    }
}

proptest! {
    // Fixed seed (same cases on every machine and run, like the other suites) and no
    // regression files: the mutant child would otherwise write one into the source tree.
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x05EE_D1A0_D15C),
        ..ProptestConfig::default()
    })]

    #[test]
    fn region_disjointness_holds_under_fuzzed_merge_split(ops in prop::collection::vec(op(), 1..80)) {
        let mut h = Harness::new();
        for op in &ops {
            h.apply(op)?;
        }
        h.apply(&Op::Tick)?;
    }
}

/// A fixed sequence that exercises every operation, so the property is known to be reached
/// with real content (not only on empty worlds) regardless of what proptest generates.
#[test]
fn a_scripted_merge_split_sequence_stays_disjoint() {
    let mut h = Harness::new();
    let script = [
        Op::Spawn { r: 0, n: 5 },
        Op::Spawn { r: 1, n: 5 },
        Op::Spawn { r: 2, n: 3 },
        Op::Tick,
        Op::Split {
            r: 0,
            mask: 0b10101,
        },
        Op::Tick,
        Op::Merge { a: 1, b: 0 },
        Op::Merge { a: 0, b: 2 }, // frame mismatch: refused
        Op::Transfer { e: 3, r: 2 },
        Op::Tick,
        Op::Split {
            r: 1,
            mask: u32::MAX,
        },
        Op::RemoveRegion { r: 0 },
        Op::Tick,
    ];
    for op in &script {
        h.apply(op).expect("property holds");
    }
    assert!(h.world.regions().len() >= 2);
}

/// Under the mutant, `split` leaves the moved entities in the source region: the scheduler's
/// own proof must refuse to run the two overlapping regions (`CORE-0007`), before any system
/// touches them. In the correct build the same steps tick normally.
#[test]
fn scheduler_refuses_overlapping_regions() {
    let mut w = World::new();
    let a = w.create_region(FrameId(0), Profile::Surface).expect("a");
    let e = w.spawn_in(a, (Pos(0), Vel(0))).expect("e");
    let mut s = Scheduler::new();
    s.add_system(Probe {
        idx: SystemIndex(0),
        kind: Kind::WritePos,
        log: Arc::default(),
        regions: Arc::default(),
    });
    // One clean tick first, so the proof below is the incremental one (journal replay), not
    // the first-tick full derivation.
    s.tick(&mut w).expect("clean tick");
    let _b = w.split_region(a, &[e]).expect("split");
    let result = s.tick(&mut w);
    if forge_core::MUTATE_DISJOINTNESS {
        let err = result.expect_err("the proof must refuse overlapping regions");
        assert_eq!(err.code().as_str(), "CORE-0007", "{err}");
        assert_eq!(
            w.get::<Pos>(e).map(|p| p.0).ok(),
            Some(1),
            "nothing ran after the split"
        );
    } else {
        result.expect("disjoint regions tick");
        assert_eq!(w.get::<Pos>(e).map(|p| p.0).ok(), Some(2));
    }
}

/// Positive control (W2): build this test with the `mutate-disjointness` bookkeeping bug and
/// assert the property test FAILS with a disjointness violation, while
/// `scheduler_refuses_overlapping_regions` passes (the proof caught the overlap too).
#[test]
fn positive_control_mutate_disjointness_build_fails() {
    if forge_core::MUTATE_DISJOINTNESS {
        return; // we ARE the mutated child; never recurse
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../target"))
        .join("mutate-disjointness");
    let out = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "test",
            "--locked",
            "-p",
            "forge-core",
            "--features",
            "mutate-disjointness",
            "--test",
            "test_region_disjointness",
            "--",
            "--test-threads=1",
            "region_disjointness_holds_under_fuzzed_merge_split",
            "scheduler_refuses_overlapping_regions",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("PROPTEST_CASES")
        .current_dir(&root)
        .output()
        .expect("cannot run cargo for the mutate-disjointness control");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "the mutated build PASSED the disjointness test — the guard is vacuous:\n{text}"
    );
    assert!(
        text.contains("test region_disjointness_holds_under_fuzzed_merge_split ... FAILED"),
        "the property test did not fail in the mutant (did it build?):\n{text}"
    );
    assert!(
        text.contains(VIOLATION),
        "the mutant failed, but not on a disjointness violation:\n{text}"
    );
    assert!(
        text.contains("test scheduler_refuses_overlapping_regions ... ok"),
        "the scheduler's own proof did not refuse the mutant's overlap:\n{text}"
    );
}
