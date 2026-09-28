//! Spike **S4**, the contention half (Appendix B, DoD M2-9):
//! `cargo run --release -p forge-core --example s4_contention`.
//!
//! The scheduler's design says a wave's jobs share **nothing mutable**: each job gets its own
//! region's entities and its own `Outbox`, and everything cross-region waits for the serial
//! barrier. This spike measures what that buys against the two designs Appendix B names as
//! fallbacks, on one workload (100,000 entities, a per-entity step of ~60 ns of integer
//! math, a warm-the-neighbour effect per region and ~2 % of entities handed to another region
//! every tick) at **4, 16 and 64 regions**, on at most 8 worker threads:
//!
//! * `forge-core serial` — the real `Scheduler::tick` (plan, incremental proof, run, barrier)
//!   on one thread: the baseline the parallel waves (M4-1) replace.
//! * `owned + barrier` — the design as built, run on threads: jobs own their region, write
//!   their own outbox, the barrier applies outboxes in (region, send) order. No lock is
//!   contended during a wave by construction.
//! * `shared queue` — the same, but every message goes into one mutex-guarded queue as it is
//!   sent (a "simpler" barrier): lock wait time is measured.
//! * `region locks` — Appendix B's first fallback: no barrier; a job applies its
//!   cross-region effects at once by locking the target region: lock wait time is measured,
//!   and effects land in thread order, not the barrier's fixed order.
//!
//! The owned design on 8 threads is checked against the same design on one thread, and the
//! shared queue against both. Workers are spawned once; a wave starts and ends on a barrier.
//! Numbers are recorded in ADR 0038.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use forge_core::bevy_ecs::component::Component;
use forge_core::{
    EntityId, Outbox, Profile, RegionAccess, RegionId, RegionSystem, RegionView, Scheduler, World,
};
use forge_frames::FrameId;

const ENTITIES: usize = 100_000;
const TICKS: usize = 30;

#[derive(Component, Clone, Copy)]
struct Pos(i64);
#[derive(Component, Clone, Copy)]
struct Vel(i64);
#[derive(Component, Clone, Copy)]
struct Heat(i64);

/// ~60 ns of dependent integer work: the stand-in for a physics step.
#[inline(never)]
fn step(p: i64, v: i64) -> i64 {
    let mut x = (p ^ v) as u64 | 1;
    for _ in 0..24 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
    }
    p.wrapping_add(v).wrapping_add((x & 3) as i64)
}

fn moves(p: i64, tick: usize) -> bool {
    ((p as u64) ^ (tick as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)).is_multiple_of(50)
}

// ---- the real scheduler, serial -------------------------------------------------------

struct Step {
    tick: usize,
    regions: Vec<RegionId>,
}
impl RegionSystem for Step {
    fn access(&self) -> RegionAccess {
        RegionAccess::new()
            .write::<Pos>()
            .read::<Vel>()
            .write::<Heat>()
    }
    fn run(&mut self, r: &mut RegionView<'_>, out: &mut Outbox) {
        let es: Vec<EntityId> = r.entities().collect();
        let me = r.id();
        let i = self.regions.iter().position(|x| *x == me).unwrap_or(0);
        let next = self.regions[(i + 1) % self.regions.len()];
        for e in es {
            let v = r.get::<Vel>(e).map_or(0, |v| v.0);
            let p = match r.get_mut::<Pos>(e) {
                Ok(mut p) => {
                    p.0 = step(p.0, v);
                    p.0
                }
                Err(_) => continue,
            };
            if moves(p, self.tick) && next != me {
                out.transfer(e, next);
            }
        }
        if next != me {
            out.send(next, |v: &mut RegionView<'_>| {
                let first = v.entities().next();
                if let Some(e) = first {
                    v.get_mut::<Heat>(e)?.0 += 1;
                }
                Ok(())
            });
        }
        self.tick += 1;
    }
}

fn serial(regions: usize) -> (Duration, u64) {
    let mut w = World::new();
    let mut ids = Vec::new();
    for _ in 0..regions {
        if let Ok(r) = w.create_region(FrameId(0), Profile::Surface) {
            ids.push(r);
        }
    }
    for i in 0..ENTITIES {
        let _ = w.spawn_in(
            ids[i % ids.len()],
            (Pos(i as i64), Vel((i % 13) as i64), Heat(0)),
        );
    }
    let mut s = Scheduler::new();
    s.add_system(Step {
        tick: 0,
        regions: ids.clone(),
    });
    let t = Instant::now();
    for _ in 0..TICKS {
        let _ = s.tick(&mut w);
    }
    let mut sum: u64 = 0;
    for r in w.regions().iter() {
        for e in r.entities() {
            sum = sum.wrapping_add(w.get::<Pos>(e).map_or(0, |p| p.0) as u64);
        }
    }
    (t.elapsed() / TICKS as u32, sum)
}

// ---- the parallel prototypes ---------------------------------------------------------

#[derive(Clone, Default)]
struct RegionData {
    pos: Vec<i64>,
    vel: Vec<i64>,
    heat: Vec<i64>,
}

enum Msg {
    Warm { to: usize },
    Transfer { from: usize, to: usize, idx: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Design {
    Owned,
    SharedQueue,
    RegionLocks,
}

#[derive(Default)]
struct Stats {
    wave: Duration,
    barrier: Duration,
    busy: Duration,
    lock_wait: Duration,
    max_job: Duration,
    jobs: usize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn build(regions: usize) -> Vec<Mutex<RegionData>> {
    let mut data = vec![RegionData::default(); regions];
    for i in 0..ENTITIES {
        let d = &mut data[i % regions];
        d.pos.push(i as i64);
        d.vel.push((i % 13) as i64);
        d.heat.push(0);
    }
    data.into_iter().map(Mutex::new).collect()
}

fn run_job(
    r: usize,
    tick: usize,
    regions: usize,
    design: Design,
    data: &[Mutex<RegionData>],
    queue: &Mutex<Vec<(usize, Msg)>>,
    outboxes: &[Mutex<Vec<Msg>>],
) -> Duration {
    let mut waited = Duration::ZERO;
    let next = (r + 1) % regions;
    let mut out: Vec<Msg> = Vec::new();
    {
        let mut d = lock(&data[r]);
        let RegionData { pos, vel, heat } = &mut *d;
        for i in 0..pos.len() {
            pos[i] = step(pos[i], vel[i]);
            heat[i] += 1;
            if moves(pos[i], tick) && next != r {
                out.push(Msg::Transfer {
                    from: r,
                    to: next,
                    idx: i,
                });
            }
        }
    }
    if next != r {
        out.push(Msg::Warm { to: next });
    }
    match design {
        Design::Owned => *lock(&outboxes[r]) = out,
        Design::SharedQueue => {
            for m in out {
                let a = Instant::now();
                let mut q = lock(queue);
                waited += a.elapsed();
                q.push((r, m));
            }
        }
        Design::RegionLocks => {
            // Effects at once, on the target's lock (no barrier: they land in thread order).
            // Transfers still need a barrier-like step, so they are queued per region.
            let mut rest = Vec::new();
            for m in out {
                match m {
                    Msg::Warm { to } => {
                        let a = Instant::now();
                        let mut t = lock(&data[to]);
                        waited += a.elapsed();
                        if let Some(h) = t.heat.first_mut() {
                            *h += 1;
                        }
                    }
                    other => rest.push(other),
                }
            }
            *lock(&outboxes[r]) = rest;
        }
    }
    waited
}

/// The serial barrier: (sending region, send order); transfers out of one region in
/// descending index order (they remove by index).
fn barrier(
    design: Design,
    data: &[Mutex<RegionData>],
    queue: &Mutex<Vec<(usize, Msg)>>,
    outboxes: &[Mutex<Vec<Msg>>],
) {
    let mut msgs: Vec<(usize, Msg)> = match design {
        Design::SharedQueue => std::mem::take(&mut *lock(queue)),
        _ => outboxes
            .iter()
            .enumerate()
            .flat_map(|(r, m)| {
                std::mem::take(&mut *lock(m))
                    .into_iter()
                    .map(move |x| (r, x))
            })
            .collect(),
    };
    msgs.sort_by(|a, b| {
        let key = |m: &(usize, Msg)| match m.1 {
            Msg::Transfer { from, idx, .. } => (m.0, from, usize::MAX - idx),
            Msg::Warm { .. } => (m.0, usize::MAX, 0),
        };
        key(a).cmp(&key(b))
    });
    for (_, m) in msgs {
        match m {
            Msg::Warm { to } => {
                if let Some(h) = lock(&data[to]).heat.first_mut() {
                    *h += 1;
                }
            }
            Msg::Transfer { from, to, idx } => {
                let (p, v, h) = {
                    let mut f = lock(&data[from]);
                    (
                        f.pos.swap_remove(idx),
                        f.vel.swap_remove(idx),
                        f.heat.swap_remove(idx),
                    )
                };
                let mut t = lock(&data[to]);
                t.pos.push(p);
                t.vel.push(v);
                t.heat.push(h);
            }
        }
    }
}

/// Run the workload on `threads` persistent workers (spawned once; a wave starts and ends on
/// a `std::sync::Barrier`, so thread start-up is not in the numbers).
fn parallel(regions: usize, threads: usize, design: Design) -> (Stats, u64) {
    use std::sync::Barrier;
    let data = build(regions);
    let queue: Mutex<Vec<(usize, Msg)>> = Mutex::new(Vec::new());
    let outboxes: Vec<Mutex<Vec<Msg>>> = (0..regions).map(|_| Mutex::new(Vec::new())).collect();
    let next_job = AtomicUsize::new(0);
    let tick_now = AtomicUsize::new(0);
    let go = Barrier::new(threads + 1);
    let done = Barrier::new(threads + 1);
    let per_worker: Vec<Mutex<(Duration, Duration, Duration, usize)>> = (0..threads)
        .map(|_| Mutex::new(Default::default()))
        .collect();
    let mut st = Stats::default();
    std::thread::scope(|s| {
        for stats in &per_worker {
            let (data, queue, outboxes, next_job, tick_now, go, done) =
                (&data, &queue, &outboxes, &next_job, &tick_now, &go, &done);
            s.spawn(move || {
                for _ in 0..TICKS {
                    go.wait();
                    let tick = tick_now.load(Ordering::Acquire);
                    loop {
                        let r = next_job.fetch_add(1, Ordering::Relaxed);
                        if r >= regions {
                            break;
                        }
                        let started = Instant::now();
                        let waited = run_job(r, tick, regions, design, data, queue, outboxes);
                        let took = started.elapsed();
                        let mut s = lock(stats);
                        s.0 += took;
                        s.1 += waited;
                        s.2 = s.2.max(took);
                        s.3 += 1;
                    }
                    done.wait();
                }
            });
        }
        for tick in 0..TICKS {
            next_job.store(0, Ordering::Release);
            tick_now.store(tick, Ordering::Release);
            let t0 = Instant::now();
            go.wait();
            done.wait();
            st.wave += t0.elapsed();
            let b0 = Instant::now();
            barrier(design, &data, &queue, &outboxes);
            st.barrier += b0.elapsed();
        }
    });
    for p in &per_worker {
        let s = lock(p);
        st.busy += s.0;
        st.lock_wait += s.1;
        st.max_job = st.max_job.max(s.2);
        st.jobs += s.3;
    }
    let n = TICKS as u32;
    st.wave /= n;
    st.barrier /= n;
    st.busy /= n;
    st.lock_wait /= n;
    let mut sum: u64 = 0;
    for d in &data {
        for p in &lock(d).pos {
            sum = sum.wrapping_add(*p as u64);
        }
    }
    (st, sum)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    println!(
        "S4 contention: {ENTITIES} entities, {TICKS} ticks, {threads} worker threads (release)\n"
    );
    println!(
        "regions | forge-core serial tick | owned+barrier: 1 thread wave -> {threads} threads wave, speed-up, barrier, lock wait | shared queue: wave, lock wait | region locks: wave, lock wait"
    );
    for regions in [4usize, 16, 64] {
        let (serial_tick, _) = serial(regions);
        let (one, one_sum) = parallel(regions, 1, Design::Owned);
        let (own, own_sum) = parallel(regions, threads, Design::Owned);
        let (shared, shared_sum) = parallel(regions, threads, Design::SharedQueue);
        let (locks, locks_sum) = parallel(regions, threads, Design::RegionLocks);
        assert_eq!(own_sum, one_sum, "threads changed the outcome");
        assert_eq!(own_sum, shared_sum, "the shared queue changed the outcome");
        assert_eq!(own_sum, locks_sum, "region locks changed the positions");
        println!(
            "{regions:>7} | {:>19.2} ms | {:>7.2} ms -> {:>5.2} ms, x{:>4.1}, {:>6.3} ms, {:>6.3} ms (max job {:.2} ms) | {:>5.2} ms, {:>6.3} ms | {:>5.2} ms, {:>6.3} ms",
            ms(serial_tick),
            ms(one.wave),
            ms(own.wave),
            ms(one.wave) / ms(own.wave).max(1e-9),
            ms(own.barrier),
            ms(own.lock_wait),
            ms(own.max_job),
            ms(shared.wave),
            ms(shared.lock_wait),
            ms(locks.wave),
            ms(locks.lock_wait),
        );
    }
}
