//! `test_gpu_mode` — the GPU mode (owner decision 2026-09-25, ADR 0054): `Single` is the
//! default and opens exactly one device even when several adapters qualify; `Multi` keeps
//! the every-adapter pool (`test_multi_adapter` asks for it explicitly).
//!
//! * Synthetic machines: in `Single` the best adapter by the selection order is used and
//!   every other qualifying adapter is `SingleMode` (duplicates, below-floor and capped
//!   adapters keep their own reasons).
//! * This machine, with the software rasteriser included so two adapters qualify on a
//!   one-GPU box (WARP on Windows, lavapipe on Linux): `Single` opens one device and reports
//!   the other as `GPU mode: Single`; `Multi` opens both. The primary is the same adapter
//!   either way, so nothing that renders on the primary changes.
//!
//! Positive control (W2): `positive_control_multi_forced_fails_the_single_guard` — the Single
//! guard applied to a `Multi` pool of two devices fails.

mod common;

use forge_gpu::wgpu::{Backend, DeviceType};
use forge_gpu::{
    AdapterFacts, AdapterPool, AdapterVerdict, ApiLevel, Choice, GpuMode, PoolOptions,
    SoftwarePolicy, plan_selection_in,
};

fn facts(name: &str, backend: Backend, ty: DeviceType, ids: (u32, u32)) -> AdapterFacts {
    AdapterFacts {
        name: name.into(),
        backend,
        device_type: ty,
        vendor: ids.0,
        device: ids.1,
        api: match backend {
            Backend::Vulkan => ApiLevel::Vulkan { major: 1, minor: 3 },
            _ => ApiLevel::Dx12 { fl_12_0: true },
        },
        compute: true,
    }
}

fn rtx(backend: Backend) -> AdapterFacts {
    facts(
        "NVIDIA GeForce RTX 3080",
        backend,
        DeviceType::DiscreteGpu,
        (0x10de, 0x2206),
    )
}

fn rx580() -> AdapterFacts {
    facts(
        "Radeon RX 580",
        Backend::Vulkan,
        DeviceType::DiscreteGpu,
        (0x1002, 0x67df),
    )
}

fn warp() -> AdapterFacts {
    facts(
        "Microsoft Basic Render Driver",
        Backend::Dx12,
        DeviceType::Cpu,
        (0x1414, 0x8c),
    )
}

/// The Single guard: one device, one `Selected` row, and every other adapter that would
/// have joined a Multi pool reported as left out by the GPU mode.
fn single_guard(pool: &AdapterPool, multi_len: usize) -> Result<(), String> {
    let verdicts: Vec<AdapterVerdict> = pool.report().iter().map(|r| r.verdict.clone()).collect();
    single_guard_on(pool.len(), &verdicts, multi_len)
}

/// [`single_guard`] on a pool's device count and report verdicts.
fn single_guard_on(
    devices: usize,
    verdicts: &[AdapterVerdict],
    multi_len: usize,
) -> Result<(), String> {
    let selected = verdicts
        .iter()
        .filter(|v| matches!(v, AdapterVerdict::Selected(_)))
        .count();
    if devices != 1 || selected != 1 {
        return Err(format!(
            "GPU mode Single must open one device; this pool opened {devices} ({selected} Selected rows)"
        ));
    }
    let left_out = verdicts
        .iter()
        .filter(|v| **v == AdapterVerdict::LeftOutByGpuMode)
        .count();
    if left_out + 1 < multi_len {
        return Err(format!(
            "{left_out} adapters reported as left out by the GPU mode; {} expected",
            multi_len - 1
        ));
    }
    Ok(())
}

/// W2: Multi forced, the Single guard fails — on this machine's Multi pool when two
/// adapters qualify here (GPU + WARP / lavapipe), and always on a Multi report of two.
#[test]
fn positive_control_multi_forced_fails_the_single_guard() {
    let two = [AdapterVerdict::Selected(0), AdapterVerdict::Selected(1)];
    assert!(single_guard_on(2, &two, 2).is_err());
    // And a Single pool that forgot to say why the second adapter is out is caught too.
    let silent = [AdapterVerdict::Selected(0), AdapterVerdict::NotRequested];
    assert!(single_guard_on(1, &silent, 2).is_err());
    let Some(multi) = common::pool(&PoolOptions {
        software: SoftwarePolicy::Include,
        mode: GpuMode::Multi,
        ..PoolOptions::default()
    }) else {
        return;
    };
    if multi.len() >= 2 {
        let e = single_guard(&multi, multi.len())
            .expect_err("the Single guard passed on a Multi pool of two devices");
        println!("positive control (Multi forced): {e}");
    } else {
        println!(
            "AWAITING(second adapter): only {} qualifies here; the control ran on a Multi report",
            multi.primary().label()
        );
    }
}

#[test]
fn the_default_is_single() {
    assert_eq!(PoolOptions::default().mode, GpuMode::Single);
}

#[test]
fn single_uses_the_best_adapter_and_leaves_the_rest_out_by_mode() {
    // Enumeration order: WARP, a second card, the RTX on D3D12 and on Vulkan.
    let f = [warp(), rx580(), rtx(Backend::Dx12), rtx(Backend::Vulkan)];
    let multi = plan_selection_in(&f, SoftwarePolicy::Include, None, GpuMode::Multi);
    assert_eq!(
        multi,
        vec![
            Choice::Use { rank: 2 },
            Choice::Use { rank: 0 },
            Choice::Duplicate { of: 3 },
            Choice::Use { rank: 1 },
        ]
    );
    let single = plan_selection_in(&f, SoftwarePolicy::Include, None, GpuMode::Single);
    assert_eq!(
        single,
        vec![
            Choice::SingleMode { rank: 2 },
            Choice::Use { rank: 0 },
            Choice::Duplicate { of: 3 },
            Choice::SingleMode { rank: 1 },
        ],
        "the same primary as Multi; the others left out by the mode, the duplicate stays a duplicate"
    );
    let uses = |p: &[Choice]| p.iter().filter(|c| matches!(c, Choice::Use { .. })).count();
    assert_eq!(uses(&single), 1);
    assert_eq!(uses(&multi), 3);
    // The device cap still applies first.
    let capped = plan_selection_in(&f, SoftwarePolicy::Include, Some(2), GpuMode::Single);
    assert_eq!(capped[0], Choice::Capped);
    assert_eq!(capped[3], Choice::SingleMode { rank: 1 });
}

#[test]
fn this_machine_single_opens_one_device_and_multi_opens_two() {
    let include = |mode| PoolOptions {
        software: SoftwarePolicy::Include,
        mode,
        ..PoolOptions::default()
    };
    let Some(multi) = common::pool(&include(GpuMode::Multi)) else {
        return;
    };
    let multi_len = multi.len();
    let multi_primary = multi.primary().label();
    for r in multi.report() {
        println!("multi : {:<60} -> {}", r.facts.label(), r.verdict.reason());
    }
    if multi_len < 2 {
        println!(
            "AWAITING(second adapter): only {multi_primary} qualifies here, so Multi opens one \
             device too; the synthetic plans and pool unit tests cover two"
        );
    }
    drop(multi);

    let Some(single) = common::pool(&include(GpuMode::Single)) else {
        return;
    };
    for r in single.report() {
        println!("single: {:<60} -> {}", r.facts.label(), r.verdict.reason());
    }
    assert_eq!(single.mode(), GpuMode::Single);
    single_guard(&single, multi_len).unwrap_or_else(|e| panic!("{e}"));
    let left_out = single
        .report()
        .iter()
        .filter(|r| r.verdict == AdapterVerdict::LeftOutByGpuMode)
        .count();
    assert_eq!(
        multi_len,
        1 + left_out,
        "Multi opens every adapter Single leaves out (the old every-adapter behaviour)"
    );
    assert_eq!(
        single.primary().label(),
        multi_primary,
        "Single must open the adapter Multi makes primary"
    );
    if multi_len >= 2 {
        assert!(
            single
                .report()
                .iter()
                .any(|r| r.verdict.reason().starts_with("GPU mode: Single")),
            "the report must say why the other adapter was left out"
        );
    }
}
