//! Scheduler skeleton cost: `cargo run --release -p forge-core --example tick_cost`.
//!
//! Measures the per-tick overhead of plan + disjointness proof + barrier with a trivial
//! system, and the cost of a real (integrate) system through `RegionView`, so later
//! changes (M4-1 parallel scheduler) have a baseline. Numbers are recorded in ADR 0005.

use std::time::Instant;

use forge_core::bevy_ecs::component::Component;
use forge_core::{
    EntityId, Outbox, Profile, RegionAccess, RegionSystem, RegionView, Scheduler, World,
};
use forge_frames::FrameId;

#[derive(Component)]
struct Pos(i64);
#[derive(Component)]
struct Vel(i64);

struct Integrate;
impl RegionSystem for Integrate {
    fn access(&self) -> RegionAccess {
        RegionAccess::new().write::<Pos>().read::<Vel>()
    }
    fn run(&mut self, r: &mut RegionView<'_>, _out: &mut Outbox) {
        let es: Vec<EntityId> = r.entities().collect();
        for e in es {
            let v = r.get::<Vel>(e).map_or(0, |v| v.0);
            if let Ok(mut p) = r.get_mut::<Pos>(e) {
                p.0 += v;
            }
        }
    }
}

struct Noop;
impl RegionSystem for Noop {
    fn access(&self) -> RegionAccess {
        RegionAccess::new()
    }
    fn run(&mut self, _r: &mut RegionView<'_>, _out: &mut Outbox) {}
}

fn world(regions: usize, per: usize) -> World {
    let mut w = World::new();
    for _ in 0..regions {
        let Ok(r) = w.create_region(FrameId(0), Profile::Surface) else {
            return w;
        };
        for i in 0..per {
            let _ = w.spawn_in(r, (Pos(0), Vel(i as i64)));
        }
    }
    w
}

fn time(label: &str, regions: usize, per: usize, sys: impl RegionSystem + 'static) {
    let mut w = world(regions, per);
    let mut s = Scheduler::new();
    s.add_system(sys);
    let _ = s.tick(&mut w); // warm
    let n = 50;
    let t = Instant::now();
    for _ in 0..n {
        let _ = s.tick(&mut w);
    }
    let per_tick = t.elapsed().as_secs_f64() / f64::from(n);
    let ents = (regions * per) as f64;
    println!(
        "{label:9} {regions:3} regions x {per:6} entities: {:9.3} ms/tick  {:7.1} ns/entity",
        per_tick * 1e3,
        per_tick * 1e9 / ents.max(1.0)
    );
}

fn main() {
    for (r, p) in [(1, 100_000), (16, 6_250), (256, 390)] {
        time("noop", r, p, Noop);
        time("integrate", r, p, Integrate);
    }
}
