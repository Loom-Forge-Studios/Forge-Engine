//! `test_mesh_churn` — adding and removing a mesh costs the same whatever the number of
//! meshes the renderer holds (owner rule 2; WP-11 verifier follow-up, WP-19).
//!
//! A streaming terrain releases patches in least-recently-used bursts and uploads new ones
//! every few frames while thousands stay resident. Before WP-18 the mesh table was a
//! copy-on-write `Arc<[_]>`: every `remove_mesh` and `add_mesh` copied the whole table, so a
//! burst of `k` removals with `n` meshes held cost O(n k). Now each call changes one slot.
//!
//! Two independent checks, because each is blind to what the other sees:
//!
//! * **Allocation** (a counting allocator, thread-local so other tests cannot pollute it):
//!   the bytes allocated by a burst of 32 removals with 4,096 meshes held must not exceed the
//!   same burst with 64 meshes held by more than a fixed allowance (less than one copy of the
//!   table). Catches a table that is copied.
//! * **Time** (WP-19): the time of a removal with 16,384 meshes held over the time with 64
//!   held must stay under [`TIME_RATIO`]. Each figure is the **minimum** over
//!   [`TIME_BURSTS`] bursts of the burst's mean (preemption and a loaded machine only ever add
//!   time, so the minimum is the cost of the code), and the measurement runs **alone**
//!   (`forge_trace::timed::run_timed_alone`: its own process, High priority, the machine-wide
//!   timed lock). Catches work proportional to the table that allocates nothing — a scan for
//!   a free slot, a live count, a validation pass — which the allocation check cannot see.
//!   An O(1) removal measures ~2.5x here (the GPU buffers' release and cache effects); a
//!   scan of 16,384 slots makes it tens of times slower.
//!
//! Positive controls (W2): `positive_control_copying_the_table_scales_with_meshes` — the
//! same burst with the table copied on every change (the pre-WP-18 behaviour, a hidden knob)
//! allocates in proportion to the meshes held and fails the allocation check;
//! `positive_control_an_allocation_free_scan_fails_the_time_check` — every removal walks the
//! whole table without allocating (a hidden knob): the allocation check passes it, the time
//! check fails it.

mod common;

use std::time::Instant;

use forge_render::{MeshData, MeshId, RenderOptions, Renderer};

const ROW: &str = "C-mesh-churn";
const BURST: usize = 32;

/// Bytes allocated by, and mean ns per removal of, a least-recently-used burst of `BURST`
/// removals with `held` meshes resident.
fn burst(held: usize, copy_table: bool) -> Option<(u64, f64)> {
    let pool = common::pool(ROW)?;
    let dev = pool.primary();
    let mut r = Renderer::new(dev, RenderOptions::new(64, 64)).expect("renderer");
    if copy_table {
        r.copy_mesh_table_on_change_for_tests();
    }
    let mesh = MeshData::cube();
    let mut ids: Vec<MeshId> = (0..held).map(|_| r.add_mesh(&mesh).expect("add")).collect();
    assert_eq!(r.mesh_count(), held);
    // Warm the free list's capacity so the measured burst sees a steady renderer.
    for id in ids.drain(held - BURST..) {
        r.remove_mesh(id).expect("remove");
    }
    for _ in 0..BURST {
        ids.push(r.add_mesh(&mesh).expect("add"));
    }
    let victims: Vec<MeshId> = ids[..BURST].to_vec();
    let t0 = Instant::now();
    let info = allocation_counter::measure(|| {
        for id in &victims {
            r.remove_mesh(*id).expect("remove");
        }
    });
    let ns = t0.elapsed().as_secs_f64() * 1e9 / BURST as f64;
    for _ in 0..BURST {
        r.add_mesh(&mesh).expect("add");
    }
    assert_eq!(r.mesh_count(), held);
    assert!(dev.take_uncaptured_errors().is_empty());
    Some((info.bytes_total, ns))
}

/// Allowed growth of the burst's allocations from 64 to 4,096 meshes held: less than one
/// copy of a 4,096-slot table.
const ALLOWANCE: u64 = 16 * 1024;

fn compare(copy_table: bool) -> Option<(u64, u64)> {
    let (small, ns_small) = burst(64, copy_table)?;
    let (large, ns_large) = burst(4096, copy_table)?;
    println!(
        "burst of {BURST} removals (copy table: {copy_table}): 64 held {small} B, \
         {ns_small:.0} ns each; 4,096 held {large} B, {ns_large:.0} ns each"
    );
    Some((small, large))
}

#[test]
fn mesh_churn_cost_is_independent_of_meshes_held() {
    let Some((small, large)) = compare(false) else {
        return;
    };
    assert!(
        large <= small + ALLOWANCE,
        "with 4,096 meshes held a burst allocated {large} B against {small} B with 64: \
         add/remove must not copy the mesh table"
    );
}

#[test]
fn positive_control_copying_the_table_scales_with_meshes() {
    let Some((small, large)) = compare(true) else {
        return;
    };
    assert!(
        large > small + ALLOWANCE,
        "copying the table on every change must scale with the meshes held ({small} B vs \
         {large} B): the comparison is blind"
    );
}

// ---- time: an allocation-free O(n) removal ------------------------------------------------

/// The most a removal with [`TIME_LARGE`] meshes held may cost over one with [`TIME_SMALL`].
const TIME_RATIO: f64 = 10.0;
const TIME_SMALL: usize = 64;
const TIME_LARGE: usize = 16_384;
const TIME_BURSTS: usize = 15;

/// The minimum over `TIME_BURSTS` bursts of the mean ns per removal, with `held` meshes held.
fn removal_ns(held: usize, scan: bool) -> Option<f64> {
    let pool = common::pool(ROW)?;
    let dev = pool.primary();
    let mut r = Renderer::new(dev, RenderOptions::new(64, 64)).expect("renderer");
    if scan {
        r.scan_mesh_table_on_remove_for_tests();
    }
    let mesh = MeshData::cube();
    let mut ids: Vec<MeshId> = (0..held).map(|_| r.add_mesh(&mesh).expect("add")).collect();
    let mut best = f64::INFINITY;
    // One warm-up burst, then the timed ones; each burst removes the oldest `BURST` meshes
    // and uploads as many again (the uploads are not timed).
    for b in 0..=TIME_BURSTS {
        let victims: Vec<MeshId> = ids.drain(..BURST).collect();
        let t0 = Instant::now();
        for id in &victims {
            r.remove_mesh(*id).expect("remove");
        }
        let ns = t0.elapsed().as_secs_f64() * 1e9 / BURST as f64;
        if b > 0 {
            best = best.min(ns);
        }
        for _ in 0..BURST {
            ids.push(r.add_mesh(&mesh).expect("add"));
        }
    }
    assert_eq!(r.mesh_count(), held);
    assert!(dev.take_uncaptured_errors().is_empty());
    Some(best)
}

/// (ns per removal with `TIME_SMALL` held, with `TIME_LARGE` held), timed alone; `None`
/// without a GPU.
fn time_ratio(scan: bool) -> Option<(f64, f64)> {
    let s = forge_trace::timed::run_timed_alone(|| {
        let (Some(small), Some(large)) =
            (removal_ns(TIME_SMALL, scan), removal_ns(TIME_LARGE, scan))
        else {
            return Ok("no GPU".into());
        };
        Ok(format!("{small} {large}"))
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let mut it = s.split(' ').map(str::parse::<f64>);
    let (Some(Ok(small)), Some(Ok(large))) = (it.next(), it.next()) else {
        println!("{ROW}: {s}");
        return None;
    };
    println!(
        "{ROW} time (scan on remove: {scan}): {small:.0} ns per removal with {TIME_SMALL} held, \
         {large:.0} ns with {TIME_LARGE} held ({:.2}x; limit {TIME_RATIO}x)",
        large / small
    );
    Some((small, large))
}

#[test]
fn a_removal_takes_the_same_time_whatever_the_meshes_held() {
    let Some((small, large)) = time_ratio(false) else {
        return;
    };
    assert!(
        large < small * TIME_RATIO,
        "a removal with {TIME_LARGE} meshes held took {large:.0} ns against {small:.0} ns with \
         {TIME_SMALL}: remove_mesh must not do work proportional to the table"
    );
}

#[test]
fn positive_control_an_allocation_free_scan_fails_the_time_check() {
    let Some((small, large)) = time_ratio(true) else {
        return;
    };
    assert!(
        large >= small * TIME_RATIO,
        "an allocation-free scan of the table on every removal measured {large:.0} ns against \
         {small:.0} ns: the time check is blind to O(meshes) work"
    );
}
