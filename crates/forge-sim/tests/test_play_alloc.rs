//! Guard (WP-19, owner rule 2): **a play step batch allocates the same whatever the number
//! of bodies** — measured with a counting allocator (thread-local, so sibling tests cannot
//! pollute it).
//!
//! A playing simulation runs a batch of fixed steps every editor frame and the viewport
//! reads the moving bodies' transforms afterwards. WP-13's play core rebuilt that whole
//! transform map after every batch (an allocation per moving body per frame) and cloned the
//! scheduler's plan every step; now the map is updated in place and the plan proven at the
//! fork is lent to the scheduler and handed back. What a batch still allocates (the
//! scheduler's per-tick bookkeeping, per region and system, and a checkpoint hash every
//! simulated second) does not grow with the bodies.
//!
//! Compared: the same four-step batch with 256 and with 4,096 moving bodies (after a warm-up
//! that sizes every reused buffer). The larger world may not allocate more often, nor more
//! than [`ALLOWANCE`] bytes more.
//!
//! Positive control (W2): `positive_control_rebuilding_the_transform_map_scales_with_bodies`
//! — the identical measurement with the WP-13 behaviour switched back on
//! (`SimFaults::rebuild_transforms`) allocates in proportion to the bodies and fails it.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use forge_cmd::{EntityKey, Value};
use forge_sim::edit::{P_ACCELERATION, P_LOCAL, P_SPIN, P_VELOCITY};
use forge_sim::{
    EditEntity, EditSnapshot, InputAction, PlayCommand, PlaySession, SimFaults, SimInput,
};
use forge_trace::{Sinks, Tracer};

/// Allowed growth of a batch's allocated bytes from 256 to 4,096 bodies: far less than one
/// map node per sixteen bodies.
const ALLOWANCE: u64 = 1024;
const STEPS: u32 = 4;

fn scene(n: u64) -> EditSnapshot {
    EditSnapshot::from_entities((0..n).map(|k| {
        let x = k as f64;
        let mut p = BTreeMap::new();
        p.insert(P_LOCAL.to_owned(), Value::Vec3([x, 0.5 * x, -x]));
        p.insert(P_VELOCITY.to_owned(), Value::Vec3([1.0, 2.0, 0.5]));
        p.insert(P_ACCELERATION.to_owned(), Value::Vec3([0.0, -9.81, 0.0]));
        p.insert(P_SPIN.to_owned(), Value::Float(15.0 + x * 1e-3));
        EditEntity {
            key: EntityKey(k),
            name: String::new(),
            parent: None,
            properties: p,
        }
    }))
}

/// Allocations (count, bytes) of one `STEPS`-step batch of a playing session of `n` bodies.
fn batch(n: u64, faults: SimFaults) -> (u64, u64) {
    let tracer: &'static Tracer = Box::leak(Box::new(Tracer::new()));
    tracer.enable(Sinks::COUNTERS).unwrap();
    let mut s = PlaySession::with_tracer(tracer).with_faults(faults);
    let edit = scene(n);
    s.control(PlayCommand::Play, &edit).unwrap();
    // An input before the warm-up: the input path is part of a frame too.
    s.input(SimInput {
        entity: EntityKey(0),
        action: InputAction::Impulse([0.0, 1.0, 0.0]),
    })
    .unwrap();
    // Warm-up: every reused buffer reaches its size. Step 3..=6 of the measured batch are
    // clear of the once-a-second checkpoint (steps 60, 120, ...).
    s.run_steps(2).unwrap();
    assert_eq!(s.transforms().len(), n as usize);
    let info = allocation_counter::measure(|| {
        s.run_steps(STEPS).unwrap();
    });
    assert_eq!(s.step(), u64::from(2 + STEPS));
    assert_eq!(s.transforms().len(), n as usize);
    (info.count_total, info.bytes_total)
}

fn compare(faults: SimFaults) -> ((u64, u64), (u64, u64)) {
    let small = batch(256, faults);
    let large = batch(4096, faults);
    println!(
        "a {STEPS}-step batch (faults {faults:?}): 256 bodies {} allocation(s) {} B, 4,096 bodies \
         {} allocation(s) {} B",
        small.0, small.1, large.0, large.1
    );
    (small, large)
}

#[test]
fn a_step_batch_allocates_the_same_whatever_the_body_count() {
    let (small, large) = compare(SimFaults::default());
    assert!(
        large.0 <= small.0 && large.1 <= small.1 + ALLOWANCE,
        "with 4,096 bodies a batch allocated {large:?} (count, bytes) against {small:?} with \
         256: a step batch must not allocate per body"
    );
}

#[test]
fn positive_control_rebuilding_the_transform_map_scales_with_bodies() {
    let (small, large) = compare(SimFaults {
        rebuild_transforms: true,
        ..SimFaults::default()
    });
    assert!(
        large.0 > small.0 || large.1 > small.1 + ALLOWANCE,
        "rebuilding the transform map every batch must scale with the bodies ({small:?} vs \
         {large:?}): the comparison is blind"
    );
}

#[test]
fn the_transform_map_follows_the_simulation_and_new_movers() {
    let tracer: &'static Tracer = Box::leak(Box::new(Tracer::new()));
    // One moving body and one at rest.
    let mut e = scene(1);
    let mut rest = BTreeMap::new();
    rest.insert(P_LOCAL.to_owned(), Value::Vec3([5.0, 0.0, 0.0]));
    e.entities.push(EditEntity {
        key: EntityKey(9),
        name: String::new(),
        parent: None,
        properties: rest,
    });
    let mut s = PlaySession::with_tracer(tracer);
    s.control(PlayCommand::Play, &e).unwrap();
    assert_eq!(s.transforms().len(), 1);
    s.run_steps(3).unwrap();
    let w = s.world().unwrap();
    assert_eq!(
        s.transforms()[&EntityKey(0)],
        w.transform_of(EntityKey(0)).unwrap(),
        "updated in place to the simulated transform"
    );
    // An input sets the resting body moving: it joins the map at the next batch.
    s.input(SimInput {
        entity: EntityKey(9),
        action: InputAction::SetVelocity([0.0, 0.0, 1.0]),
    })
    .unwrap();
    s.run_steps(2).unwrap();
    let w = s.world().unwrap();
    assert_eq!(s.transforms().len(), 2);
    assert_eq!(s.transforms(), &w.moving_transforms());
}
