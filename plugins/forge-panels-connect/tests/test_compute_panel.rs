//! `test_compute_panel` (Ch.21 §21.21 "Compute and farm", Ch.26; DoD M2-56), headless over
//! the labelled in-memory pool (D-4 until `forge-jobs` / `forge-farm`):
//!
//! * adapters are listed with their **measured** throughput ("not measured yet" until a job
//!   reports — never a guess from the name); farm nodes with discovery, status and failures;
//! * a live panel: a producer reporting at 100 Hz for 10 s refreshes it at most twice a
//!   second, and an idle pool costs nothing.
//!
//! Positive control (W2): `positive_control_an_uncapped_compute_panel_fails` registers the
//! pool's feed at 60 Hz; the ≤ 2 Hz check must fail.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{feed, part, rows};
use forge_editor::connect::compute::{AdapterInfo, ComputePool, MemoryComputePool, NodeStatus};
use forge_editor::testing::Rig;

const C: &str = "forge.compute";

fn rig(uncapped: bool) -> (Rig, Arc<MemoryComputePool>) {
    let pool = Arc::new(MemoryComputePool::new());
    let p = Arc::clone(&pool);
    let rig = common::rig_with(&[C], None, move |cfg, _| {
        cfg.services.connect.compute = p;
        cfg.services.connect.faults.compute_uncapped = uncapped;
    });
    (rig, pool)
}

fn list(rig: &mut Rig, group: &str, name: &str) -> Vec<String> {
    let id = part(rig, C, &["content", group, name]);
    rows(rig, id)
}

#[test]
fn adapters_show_measured_throughput_and_nodes_their_status_and_failures() {
    let (mut rig, pool) = rig(false);
    pool.set_adapters(vec![
        AdapterInfo {
            name: "RTX 3080".into(),
            kind: "discrete".into(),
            backend: "Vulkan".into(),
            items_per_s: None,
            jobs_done: 0,
            enabled: true,
        },
        AdapterInfo {
            name: "UHD 630".into(),
            kind: "integrated".into(),
            backend: "D3D12".into(),
            items_per_s: None,
            jobs_done: 0,
            enabled: true,
        },
    ]);
    feed(&mut rig);
    let a = list(&mut rig, "adapters_group", "adapters");
    assert!(
        a.iter()
            .all(|l| l.contains("not measured yet") && l.contains("0 job(s)")),
        "{a:?}"
    );
    pool.report_job("RTX 3080", 4_000_000, 0.5);
    pool.set_discovering(true);
    pool.node("farm-1", "192.168.1.20", NodeStatus::Idle, Some(2.5e6));
    pool.node("farm-2", "192.168.1.21", NodeStatus::Busy { jobs: 3 }, None);
    pool.node(
        "farm-3",
        "192.168.1.22",
        NodeStatus::Lost { since_ms: 1 },
        None,
    );
    pool.failure("farm-3", "erosion job 17: node stopped answering");
    feed(&mut rig);
    let a = list(&mut rig, "adapters_group", "adapters");
    assert!(
        a.iter().any(|l| l.starts_with("RTX 3080")
            && l.contains("8.00 M items/s")
            && l.contains("1 job(s)")),
        "{a:?}"
    );
    let n = list(&mut rig, "nodes_group", "nodes");
    assert!(
        n.iter()
            .any(|l| l.starts_with("farm-1 at 192.168.1.20") && l.contains("idle")),
        "{n:?}"
    );
    assert!(n.iter().any(|l| l.contains("busy (3 job(s))")), "{n:?}");
    assert!(
        n.iter()
            .any(|l| l.starts_with("farm-3") && l.contains("lost") && l.contains("1 failure(s)")),
        "{n:?}"
    );
    assert!(
        n.iter().any(|l| l.contains("erosion job 17")),
        "the failure is listed under its node: {n:?}"
    );
    let head = common::label(&rig, part(&rig, C, &["content", "head"]));
    assert!(
        head.contains("3 farm node(s)") && head.contains("1 lost") && head.contains("discovering"),
        "{head}"
    );
    let footer = common::label(&rig, part(&rig, C, &["content", "footer"]));
    assert!(footer.contains("UNBUILT"), "{footer}");
}

fn check_rate(uncapped: bool) -> Result<(), String> {
    let (mut rig, pool) = rig(uncapped);
    rig.advance(Duration::from_secs(6));
    // Idle: nothing reported, nothing drawn, no wakeups.
    let (frames, wakeups) = rig.advance(Duration::from_secs(5));
    if (frames, wakeups) != (0, 0) {
        return Err(format!(
            "an idle pool drew {frames} frames, {wakeups} wakeups"
        ));
    }
    let (f0, _) = (rig.h.frames, rig.h.wakeups);
    for i in 0..1000u64 {
        let name = pool.snapshot().adapters[0].name.clone();
        pool.report_job(&name, 1000 + i, 0.01);
        rig.advance(Duration::from_millis(10));
    }
    pool.set_idle();
    let frames = rig.h.frames - f0;
    if frames > 2 * 10 + 1 {
        return Err(format!(
            "the compute panel drew {frames} frames in 10 s (cap 2 Hz)"
        ));
    }
    if frames < 5 {
        return Err(format!(
            "the compute panel drew only {frames} frames in 10 s"
        ));
    }
    Ok(())
}

#[test]
fn the_compute_panel_refreshes_at_most_twice_a_second() {
    check_rate(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_uncapped_compute_panel_fails() {
    let e = check_rate(true).expect_err("an uncapped compute panel must fail the 2 Hz cap");
    assert!(e.contains("cap 2 Hz"), "{e}");
}
