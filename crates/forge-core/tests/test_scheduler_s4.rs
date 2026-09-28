// timed-gates: exempt(ms per tick is printed for the record; the asserts are on conservation and determinism)
//! Spike **S4** (Appendix B, DoD M2-9): the regionized scheduler **fuzzed under load at 4, 16
//! and 64 regions** — merges and splits every tick, transfers, spawns and despawns between
//! ticks, and three systems a tick that write components, send barrier effects to other
//! regions and hand entities across region boundaries.
//!
//! After every tick, independently of the scheduler's own incremental proof:
//!
//! 1. the plan and the ownership table verify from scratch (`Scheduler::verify`, `CORE-0007`
//!    / `CORE-0008` otherwise);
//! 2. **conservation**: every live entity is owned by exactly one region, and the regions
//!    hold nothing else;
//! 3. every barrier message applied (none rejected: a transfer of an entity the sender no
//!    longer owns would be a bookkeeping bug);
//!
//! and across runs: the same entity workload under 4, 16 and 64 regions, each with its own
//! merge/split churn, ends with **identical integrated state** — the region layout is a
//! scheduling decision and must never change an outcome (I5 applied to regions).
//!
//! The contention half of S4 (parallel waves, shared-queue and per-region-lock designs) is
//! measured by `cargo run --release -p forge-core --example s4_contention`; both results are
//! recorded in ADR 0038.
//!
//! Positive control (W2): `positive_control_mutate_disjointness_fails_s4` rebuilds this file
//! with `--features mutate-disjointness` (`split` leaves moved entities in the source region
//! too) and asserts the fuzz FAILS there on an S4 violation.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_core::bevy_ecs::component::Component;
use forge_core::{
    EntityId, Outbox, Profile, RegionAccess, RegionId, RegionSystem, RegionView, Scheduler, World,
};
use forge_frames::FrameId;

const VIOLATION: &str = "S4 VIOLATION";

#[derive(Component, Clone, Copy)]
struct Pos(i64);
#[derive(Component, Clone, Copy)]
struct Vel(i64);
#[derive(Component, Clone, Copy)]
struct Heat(i64);

/// SplitMix64: a deterministic stream.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// pos += vel: the region-independent part of the workload.
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
                p.0 = p.0.wrapping_add(v.0);
            }
        }
    }
}

/// Every entity warms up; each region also warms the first entity of the next region at the
/// barrier (a cross-region effect every tick, from every region).
struct Warm {
    regions: Arc<Mutex<Vec<RegionId>>>,
}
impl RegionSystem for Warm {
    fn access(&self) -> RegionAccess {
        RegionAccess::new().write::<Heat>()
    }
    fn run(&mut self, r: &mut RegionView<'_>, out: &mut Outbox) {
        let es: Vec<EntityId> = r.entities().collect();
        for e in &es {
            if let Ok(mut h) = r.get_mut::<Heat>(*e) {
                h.0 += 1;
            }
        }
        let ids = self.regions.lock().map(|v| v.clone()).unwrap_or_default();
        if let Some(i) = ids.iter().position(|x| *x == r.id())
            && let Some(next) = ids.get((i + 1) % ids.len()).copied()
            && next != r.id()
        {
            out.send(next, |v: &mut RegionView<'_>| {
                let first = v.entities().next();
                if let Some(e) = first {
                    let mut h = v.get_mut::<Heat>(e)?;
                    h.0 += 1;
                }
                Ok(())
            });
        }
    }
}

/// Hands about one entity in 50 to another region at the barrier.
struct Migrate {
    regions: Arc<Mutex<Vec<RegionId>>>,
    tick: u64,
}
impl RegionSystem for Migrate {
    fn access(&self) -> RegionAccess {
        RegionAccess::new().read::<Pos>()
    }
    fn run(&mut self, r: &mut RegionView<'_>, out: &mut Outbox) {
        self.tick += 1;
        let ids = self.regions.lock().map(|v| v.clone()).unwrap_or_default();
        if ids.len() < 2 {
            return;
        }
        let es: Vec<EntityId> = r.entities().collect();
        for e in es {
            let p = r.get::<Pos>(e).map_or(0, |p| p.0);
            let h = (p as u64) ^ self.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            if h.is_multiple_of(50) {
                let to = ids[(h as usize / 50) % ids.len()];
                if to != r.id() {
                    out.transfer(e, to);
                }
            }
        }
    }
}

struct Run {
    /// Hash of every live entity's integrated position, in spawn order.
    pos_hash: u64,
    ticks: u64,
    merges: u64,
    splits: u64,
    transfers_applied: usize,
    ms_per_tick: f64,
}

fn fail(what: String) -> String {
    format!("{VIOLATION}: {what}")
}

/// One fuzzed run at about `target` regions. The entity workload comes from `entity_seed`
/// alone (identical across region counts); the region churn from `target`.
fn run(target: usize, entity_seed: u64, ticks: u64) -> Result<Run, String> {
    let mut w = World::new();
    let mut ents = Rng(entity_seed);
    let mut churn = Rng(entity_seed ^ (target as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
    let mut live: Vec<EntityId> = Vec::new();
    let mut regions: Vec<RegionId> = Vec::new();
    for _ in 0..target {
        regions.push(
            w.create_region(FrameId(0), Profile::Surface)
                .map_err(|e| e.to_string())?,
        );
    }
    // A region in another frame: it must never be merged with the rest.
    let other = w
        .create_region(FrameId(1), Profile::Surface)
        .map_err(|e| e.to_string())?;
    for i in 0..2048u64 {
        let r = regions[(i as usize) % regions.len()];
        let e = w
            .spawn_in(r, (Pos(0), Vel((ents.next() % 17) as i64 - 8), Heat(0)))
            .map_err(|e| e.to_string())?;
        live.push(e);
    }
    let shared = Arc::new(Mutex::new(Vec::new()));
    let mut s = Scheduler::new();
    s.add_system(Integrate);
    s.add_system(Warm {
        regions: Arc::clone(&shared),
    });
    s.add_system(Migrate {
        regions: Arc::clone(&shared),
        tick: 0,
    });
    let (mut merges, mut splits, mut applied) = (0u64, 0u64, 0usize);
    let start = std::time::Instant::now();
    for tick in 0..ticks {
        // ---- churn between ticks: merge on proximity, split on load ----
        let same_frame: Vec<RegionId> = w
            .regions()
            .iter()
            .filter(|r| r.frame() == FrameId(0))
            .map(forge_core::Region::id)
            .collect();
        for _ in 0..3 {
            let n = w.regions().len();
            if n > target + 1 && same_frame.len() >= 2 {
                let a = same_frame[churn.below(same_frame.len())];
                let b = same_frame[churn.below(same_frame.len())];
                if a != b && w.region(a).is_ok() && w.region(b).is_ok() {
                    w.merge_regions(a, b)
                        .map_err(|e| fail(format!("merge: {e}")))?;
                    merges += 1;
                }
            } else {
                let pick = same_frame[churn.below(same_frame.len())];
                let Ok(reg) = w.region(pick) else { continue };
                let members: Vec<EntityId> = reg.entities().collect();
                if members.len() < 4 {
                    continue;
                }
                let mask = churn.next();
                let half: Vec<EntityId> = members
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask >> (i % 64) & 1 == 1)
                    .map(|(_, e)| *e)
                    .collect();
                if !half.is_empty() && half.len() < members.len() {
                    w.split_region(pick, &half)
                        .map_err(|e| fail(format!("split: {e}")))?;
                    splits += 1;
                }
            }
        }
        // A cross-frame merge is refused (CORE-0004) and changes nothing.
        if let Some(&a) = same_frame.first()
            && w.merge_regions(a, other).is_ok()
        {
            return Err(fail("a merge across frames was accepted".into()));
        }
        // Entity churn (from the entity stream only: identical at every region count).
        let ids: Vec<RegionId> = w.regions().ids().collect();
        if ents.below(4) == 0 && !live.is_empty() {
            let i = ents.below(live.len());
            let e = live.remove(i);
            w.despawn(e).map_err(|x| fail(format!("despawn: {x}")))?;
        }
        if ents.below(4) == 0 {
            let r = ids[churn.below(ids.len())];
            let v = (ents.next() % 17) as i64 - 8;
            live.push(
                w.spawn_in(r, (Pos(0), Vel(v), Heat(0)))
                    .map_err(|e| e.to_string())?,
            );
        }
        if let Ok(mut g) = shared.lock() {
            *g = w.regions().ids().collect();
        }

        // ---- the tick, and the checks ----
        let plan = s.plan(&w);
        s.verify(&plan, &w)
            .map_err(|e| fail(format!("tick {tick}: the plan does not verify: {e}")))?;
        let report = s
            .tick_with_plan(&mut w, plan)
            .map_err(|e| fail(format!("tick {tick}: the scheduler refused: {e}")))?;
        if !report.rejected.is_empty() {
            return Err(fail(format!(
                "tick {tick}: {} barrier message(s) rejected, first: {}",
                report.rejected.len(),
                report.rejected[0]
            )));
        }
        applied += report.applied;
        let mut owned = BTreeSet::new();
        let mut total = 0usize;
        for r in w.regions().iter() {
            total += r.len();
            owned.extend(r.entities());
        }
        let alive: BTreeSet<EntityId> = live.iter().copied().collect();
        if total != owned.len() || owned != alive {
            return Err(fail(format!(
                "tick {tick}: conservation broken: {} live, {} owned, {total} memberships",
                alive.len(),
                owned.len()
            )));
        }
        w.regions()
            .check_invariants()
            .map_err(|e| fail(format!("tick {tick}: {e}")))?;
    }
    let ms_per_tick = start.elapsed().as_secs_f64() * 1000.0 / ticks as f64;
    let mut h = DefaultHasher::new();
    for e in &live {
        w.get::<Pos>(*e)
            .map(|p| p.0)
            .unwrap_or(i64::MIN)
            .hash(&mut h);
    }
    Ok(Run {
        pos_hash: h.finish(),
        ticks,
        merges,
        splits,
        transfers_applied: applied,
        ms_per_tick,
    })
}

#[test]
fn s4_scheduler_fuzzed_under_load_at_4_16_64_regions() {
    let mut hashes = Vec::new();
    for target in [4usize, 16, 64] {
        for seed in [1u64, 0xF0F0_1234] {
            let r = run(target, seed, 120).unwrap_or_else(|e| panic!("{target} regions: {e}"));
            println!(
                "S4 fuzz: {target:2} regions, seed {seed:#x}: {} ticks, {} merges, {} splits, {} barrier messages applied, {:.3} ms/tick (debug)",
                r.ticks, r.merges, r.splits, r.transfers_applied, r.ms_per_tick
            );
            assert!(r.merges > 0 && r.splits > 0, "the churn did not happen");
            assert!(r.transfers_applied > 1000, "the load did not happen");
            hashes.push((seed, target, r.pos_hash));
        }
    }
    for seed in [1u64, 0xF0F0_1234] {
        let per: Vec<(usize, u64)> = hashes
            .iter()
            .filter(|(s, ..)| *s == seed)
            .map(|(_, t, h)| (*t, *h))
            .collect();
        assert!(
            per.windows(2).all(|w| w[0].1 == w[1].1),
            "{VIOLATION}: the integrated state depends on the region count: {per:?}"
        );
    }
}

#[test]
fn positive_control_mutate_disjointness_fails_s4() {
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
            "test_scheduler_s4",
            "--",
            "--test-threads=1",
            "s4_scheduler_fuzzed_under_load_at_4_16_64_regions",
        ])
        .env("CARGO_TARGET_DIR", &target)
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
        "the mutated build PASSED the S4 fuzz — the guard is vacuous:\n{text}"
    );
    assert!(
        text.contains("test s4_scheduler_fuzzed_under_load_at_4_16_64_regions ... FAILED"),
        "the S4 fuzz did not fail in the mutant (did it build?):\n{text}"
    );
    assert!(
        text.contains(VIOLATION),
        "the mutant failed, but not on an S4 violation:\n{text}"
    );
}
