//! `test_adapter_pool` — the adapter pool and the O-8 floor (Ch.9, M1-1).
//!
//! * The floor refuses every class of adapter O-8 excludes, each with a user-facing reason.
//! * Selection counts one physical GPU once across backends, keeps two identical cards as
//!   two, orders discrete before integrated before software, applies the software policy
//!   and the device cap — on synthetic machines (a mismatched second card included).
//! * On this machine: every adapter the driver lists appears in the report with a verdict,
//!   the probes read real API levels, and the primary is a hardware adapter above the floor.
//! * GPU mode Single skips the Direct3D 12 enumeration (WP-48, ADR 0065): every *enumerated*
//!   adapter is reported, and the skip happens exactly when Vulkan yields a discrete adapter
//!   above the floor ([`skip_guard`], on this machine against the full Multi enumeration).
//!
//! Positive control (W2): `positive_control_every_excluded_class_is_refused` feeds one
//! adapter of each excluded class and asserts each is refused with its reason; the real
//! D3D12 probe is shown able to answer "no" (`the_dx12_probe_can_say_no`);
//! `positive_control_the_skip_guard_catches_both_sides` shows the skip guard failing on a skip
//! without a qualifying Vulkan adapter, a missed skip, a lost adapter and a skipped backend's
//! row.

mod common;

use forge_gpu::floor::{FLOOR_TEXT, below_floor_message};
use forge_gpu::wgpu::{Backend, Backends, DeviceType};
use forge_gpu::{
    AdapterFacts, AdapterPool, AdapterReport, AdapterVerdict, ApiLevel, Choice, GpuMode,
    PoolOptions, SoftwarePolicy, check_floor, plan_selection, vulkan_suffices,
};

fn facts(
    name: &str,
    backend: Backend,
    ty: DeviceType,
    ids: (u32, u32),
    api: ApiLevel,
) -> AdapterFacts {
    AdapterFacts {
        name: name.into(),
        backend,
        device_type: ty,
        vendor: ids.0,
        device: ids.1,
        api,
        compute: true,
    }
}

const VK13: ApiLevel = ApiLevel::Vulkan { major: 1, minor: 3 };
const VK11: ApiLevel = ApiLevel::Vulkan { major: 1, minor: 1 };
const FL12: ApiLevel = ApiLevel::Dx12 { fl_12_0: true };
const FL11: ApiLevel = ApiLevel::Dx12 { fl_12_0: false };

fn rtx(backend: Backend) -> AdapterFacts {
    let api = if backend == Backend::Vulkan {
        VK13
    } else {
        FL12
    };
    facts(
        "NVIDIA GeForce RTX 3080",
        backend,
        DeviceType::DiscreteGpu,
        (0x10de, 0x2206),
        api,
    )
}

fn warp() -> AdapterFacts {
    facts(
        "Microsoft Basic Render Driver",
        Backend::Dx12,
        DeviceType::Cpu,
        (0x1414, 0x8c),
        FL12,
    )
}

#[test]
fn positive_control_every_excluded_class_is_refused() {
    let mut no_compute = rtx(Backend::Vulkan);
    no_compute.compute = false;
    let cases = [
        (
            facts(
                "GTX 660",
                Backend::Dx12,
                DeviceType::DiscreteGpu,
                (0x10de, 0x11c0),
                FL11,
            ),
            "feature level 12_0",
        ),
        (
            facts(
                "old driver",
                Backend::Vulkan,
                DeviceType::DiscreteGpu,
                (1, 2),
                VK11,
            ),
            "Vulkan 1.2",
        ),
        (
            facts(
                "GL context",
                Backend::Gl,
                DeviceType::IntegratedGpu,
                (1, 3),
                ApiLevel::Gl,
            ),
            "OpenGL is not supported",
        ),
        (
            facts(
                "Apple M2",
                Backend::Metal,
                DeviceType::IntegratedGpu,
                (1, 4),
                ApiLevel::Metal,
            ),
            "not a supported platform",
        ),
        (
            facts(
                "mystery",
                Backend::Noop,
                DeviceType::Other,
                (1, 5),
                ApiLevel::Unknown,
            ),
            "could not be determined",
        ),
        (no_compute, "no compute shaders"),
    ];
    for (f, why) in &cases {
        let miss = check_floor(f).expect_err(&format!("{} must be refused", f.label()));
        assert!(
            miss.reasons.iter().any(|r| r.contains(why)),
            "{}: reasons {:?} should mention {why:?}",
            f.label(),
            miss.reasons
        );
        // And selection never uses it, whatever the policy.
        for policy in [
            SoftwarePolicy::Include,
            SoftwarePolicy::FallbackOnly,
            SoftwarePolicy::Only,
        ] {
            let plan = plan_selection(std::slice::from_ref(f), policy, None);
            assert!(
                matches!(plan[0], Choice::BelowFloor(_)),
                "{policy:?}: {plan:?}"
            );
        }
    }
    // The boundary itself is admitted.
    assert!(
        check_floor(&facts(
            "v12",
            Backend::Vulkan,
            DeviceType::DiscreteGpu,
            (1, 6),
            ApiLevel::Vulkan { major: 1, minor: 2 }
        ))
        .is_ok()
    );
    assert!(check_floor(&rtx(Backend::Dx12)).is_ok());
}

#[test]
fn the_below_floor_message_is_for_the_user() {
    let misses: Vec<_> = [
        facts(
            "GTX 660",
            Backend::Dx12,
            DeviceType::DiscreteGpu,
            (0x10de, 0x11c0),
            FL11,
        ),
        facts(
            "GTX 660",
            Backend::Vulkan,
            DeviceType::DiscreteGpu,
            (0x10de, 0x11c0),
            VK11,
        ),
    ]
    .iter()
    .filter_map(|f| check_floor(f).err())
    .collect();
    let m = below_floor_message(&misses);
    assert!(m.contains(FLOOR_TEXT), "{m}");
    assert!(
        m.contains("GTX 660 (Direct3D 12, feature level 11_x)"),
        "{m}"
    );
    assert!(m.contains("GTX 660 (Vulkan 1.1)"), "{m}");
    assert!(m.contains("Update the graphics driver"), "{m}");
    let e = forge_gpu::GpuError::BelowFloor { message: m };
    assert!(
        e.to_string()
            .starts_with("GPU-0002: Forge cannot start the GPU")
    );
}

#[test]
fn one_physical_gpu_on_two_backends_is_counted_once() {
    // This machine's shape: an RTX 3080 on Vulkan and on D3D12, and WARP.
    let f = [rtx(Backend::Dx12), rtx(Backend::Vulkan), warp()];
    let plan = plan_selection(&f, SoftwarePolicy::FallbackOnly, None);
    assert_eq!(
        plan,
        vec![
            Choice::Duplicate { of: 1 },
            Choice::Use { rank: 0 },
            Choice::NotRequested
        ],
        "Vulkan preferred for the same part; WARP only as a fallback"
    );
    let plan = plan_selection(&f, SoftwarePolicy::Include, None);
    assert_eq!(plan[2], Choice::Use { rank: 1 });
    let plan = plan_selection(&f, SoftwarePolicy::Only, None);
    assert_eq!(plan[1], Choice::NotRequested);
    assert_eq!(plan[2], Choice::Use { rank: 0 });
}

#[test]
fn two_identical_cards_stay_two() {
    let f = [
        rtx(Backend::Vulkan),
        rtx(Backend::Vulkan),
        rtx(Backend::Dx12),
        rtx(Backend::Dx12),
    ];
    let plan = plan_selection(&f, SoftwarePolicy::FallbackOnly, None);
    assert_eq!(
        plan,
        vec![
            Choice::Use { rank: 0 },
            Choice::Use { rank: 1 },
            Choice::Duplicate { of: 0 },
            Choice::Duplicate { of: 1 },
        ]
    );
}

#[test]
fn a_mismatched_second_card_joins_the_pool_and_a_below_floor_one_does_not() {
    let rx580_vk = facts(
        "Radeon RX 580",
        Backend::Vulkan,
        DeviceType::DiscreteGpu,
        (0x1002, 0x67df),
        VK13,
    );
    let rx580_dx = facts(
        "Radeon RX 580 Series",
        Backend::Dx12,
        DeviceType::DiscreteGpu,
        (0x1002, 0x67df),
        FL12,
    );
    let gtx660 = facts(
        "GTX 660",
        Backend::Dx12,
        DeviceType::DiscreteGpu,
        (0x10de, 0x11c0),
        FL11,
    );
    let igpu = facts(
        "Intel UHD 770",
        Backend::Vulkan,
        DeviceType::IntegratedGpu,
        (0x8086, 0x4680),
        VK13,
    );
    // Enumeration order deliberately puts the iGPU first.
    let f = [
        igpu,
        rx580_dx,
        gtx660,
        rtx(Backend::Vulkan),
        rx580_vk,
        rtx(Backend::Dx12),
        warp(),
    ];
    let plan = plan_selection(&f, SoftwarePolicy::FallbackOnly, None);
    assert_eq!(
        plan[0],
        Choice::Use { rank: 2 },
        "integrated after both discrete cards"
    );
    assert_eq!(
        plan[1],
        Choice::Duplicate { of: 4 },
        "names differ across backends; ids match"
    );
    assert!(matches!(plan[2], Choice::BelowFloor(_)));
    assert_eq!(plan[3], Choice::Use { rank: 0 });
    assert_eq!(plan[4], Choice::Use { rank: 1 });
    assert_eq!(plan[5], Choice::Duplicate { of: 3 });
    assert_eq!(plan[6], Choice::NotRequested);
    let capped = plan_selection(&f, SoftwarePolicy::FallbackOnly, Some(2));
    assert_eq!(capped[0], Choice::Capped);
}

#[test]
fn a_software_only_box_falls_back_and_exclude_refuses() {
    let f = [warp()];
    assert_eq!(
        plan_selection(&f, SoftwarePolicy::FallbackOnly, None),
        vec![Choice::Use { rank: 0 }]
    );
    assert_eq!(
        plan_selection(&f, SoftwarePolicy::Exclude, None),
        vec![Choice::NotRequested]
    );
}

#[test]
fn this_machine_every_adapter_is_reported_and_the_primary_meets_the_floor() {
    let Some(pool) = common::pool(&PoolOptions {
        software: SoftwarePolicy::Include,
        mode: GpuMode::Multi,
        ..PoolOptions::default()
    }) else {
        return;
    };
    for r in pool.report() {
        println!(
            "{:<64} {:?} -> {:?}",
            r.facts.label(),
            r.facts.backend,
            r.verdict
        );
    }
    let selected = pool
        .report()
        .iter()
        .filter(|r| matches!(r.verdict, AdapterVerdict::Selected(_)))
        .count();
    assert_eq!(
        selected,
        pool.len(),
        "every device has exactly one Selected row"
    );
    for (i, d) in pool.devices().iter().enumerate() {
        assert_eq!(d.index, i);
        assert!(
            check_floor(&d.facts).is_ok(),
            "{} is below the floor",
            d.label()
        );
        assert!(
            !matches!(d.facts.api, ApiLevel::Unknown),
            "{}: the probe read no API level",
            d.label()
        );
    }
    let primary = pool.primary();
    let hardware = pool
        .report()
        .iter()
        .any(|r| !r.facts.is_software() && check_floor(&r.facts).is_ok());
    if hardware {
        assert!(
            !primary.facts.is_software(),
            "hardware exists but the primary is {}",
            primary.label()
        );
    }
    let physical: std::collections::BTreeSet<(u32, u32)> = pool
        .devices()
        .iter()
        .filter(|d| !d.facts.is_software())
        .map(|d| (d.facts.vendor, d.facts.device))
        .collect();
    if physical.len() < 2 {
        println!(
            "AWAITING(second physical adapter): {} hardware adapter(s) in the pool",
            physical.len()
        );
    }
    // The default policy leaves the software rasteriser out when hardware exists.
    let default = AdapterPool::new(&PoolOptions::default()).expect("default pool");
    if hardware {
        assert!(default.devices().iter().all(|d| !d.facts.is_software()));
    }
    assert!(default.primary().take_uncaptured_errors().is_empty());
}

/// `(label, backend)` of each row, sorted: a report as a multiset.
fn rows(report: &[AdapterReport]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = report
        .iter()
        .map(|r| (r.facts.label(), format!("{:?}", r.facts.backend)))
        .collect();
    v.sort();
    v
}

/// The GPU-mode-Single skip guard. `full` is the report of the full enumeration (GPU mode
/// Multi, the same software policy), `single` and `skipped` the Single pool's report and
/// skipped backends, `wanted` the backends the pool was asked for.
///
/// * Every enumerated adapter is reported: `single`'s rows are exactly `full`'s rows less
///   those of the skipped backends, and none of a skipped backend.
/// * The skip happens only when a qualifying Vulkan adapter exists (a discrete GPU above the
///   floor, [`vulkan_suffices`]) and, so the speed-up is never silently lost, whenever one
///   does and something besides Vulkan was wanted.
fn skip_guard(
    full: &[AdapterReport],
    single: &[AdapterReport],
    skipped: Backends,
    wanted: Backends,
) -> Result<(), String> {
    let skipped_row = |r: &AdapterReport| skipped.contains(Backends::from(r.facts.backend));
    if let Some(r) = single.iter().find(|r| skipped_row(r)) {
        return Err(format!(
            "{} is reported although its backend {:?} was skipped",
            r.facts.label(),
            r.facts.backend
        ));
    }
    let expected: Vec<AdapterReport> = full.iter().filter(|r| !skipped_row(r)).cloned().collect();
    if rows(&expected) != rows(single) {
        return Err(format!(
            "the Single report does not list every enumerated adapter: expected {:?}, got {:?}",
            rows(&expected),
            rows(single)
        ));
    }
    let vulkan: Vec<AdapterFacts> = full
        .iter()
        .filter(|r| r.facts.backend == Backend::Vulkan)
        .map(|r| r.facts.clone())
        .collect();
    let qualifies = vulkan_suffices(&vulkan, SoftwarePolicy::FallbackOnly, GpuMode::Single);
    if !skipped.is_empty() && !qualifies {
        return Err(format!(
            "{skipped:?} was skipped but Vulkan yields no discrete adapter above the floor: {:?}",
            rows(full)
        ));
    }
    let others = wanted.difference(Backends::VULKAN);
    if qualifies && skipped.is_empty() && wanted.contains(Backends::VULKAN) && !others.is_empty() {
        return Err(format!(
            "Vulkan yields a qualifying discrete adapter but {others:?} was enumerated anyway"
        ));
    }
    Ok(())
}

#[test]
fn vulkan_suffices_only_for_a_discrete_vulkan_gpu_above_the_floor_in_single() {
    let single =
        |f: &[AdapterFacts]| vulkan_suffices(f, SoftwarePolicy::FallbackOnly, GpuMode::Single);
    assert!(single(&[rtx(Backend::Vulkan)]));
    assert!(vulkan_suffices(
        &[rtx(Backend::Vulkan)],
        SoftwarePolicy::Exclude,
        GpuMode::Single
    ));
    // Multi always enumerates everything; Include and Only want WARP (a D3D12 adapter).
    assert!(!vulkan_suffices(
        &[rtx(Backend::Vulkan)],
        SoftwarePolicy::FallbackOnly,
        GpuMode::Multi
    ));
    for policy in [SoftwarePolicy::Include, SoftwarePolicy::Only] {
        assert!(!vulkan_suffices(
            &[rtx(Backend::Vulkan)],
            policy,
            GpuMode::Single
        ));
    }
    // No Vulkan adapter, an integrated one, one below the floor, or lavapipe: D3D12 may hold
    // a better adapter, and WARP is the fallback.
    assert!(!single(&[]));
    assert!(!single(&[rtx(Backend::Dx12)]));
    let igpu = facts(
        "Intel UHD 770",
        Backend::Vulkan,
        DeviceType::IntegratedGpu,
        (0x8086, 0x4680),
        VK13,
    );
    assert!(!single(std::slice::from_ref(&igpu)));
    let old = facts(
        "old driver",
        Backend::Vulkan,
        DeviceType::DiscreteGpu,
        (1, 2),
        VK11,
    );
    assert!(!single(&[old, igpu]));
    let lavapipe = facts(
        "llvmpipe (LLVM 17.0.6, 256 bits)",
        Backend::Vulkan,
        DeviceType::Cpu,
        (0x10005, 0),
        VK13,
    );
    assert!(!single(&[lavapipe]));
}

fn report(f: AdapterFacts, verdict: AdapterVerdict) -> AdapterReport {
    AdapterReport { facts: f, verdict }
}

/// This machine's full enumeration (the RTX 3080 on Vulkan and D3D12, WARP), as a report.
fn full_report(vulkan: AdapterFacts) -> Vec<AdapterReport> {
    vec![
        report(vulkan.clone(), AdapterVerdict::Selected(0)),
        report(
            rtx(Backend::Dx12),
            AdapterVerdict::Duplicate { of: vulkan.label() },
        ),
        report(warp(), AdapterVerdict::NotRequested),
    ]
}

#[test]
fn positive_control_the_skip_guard_catches_both_sides() {
    let wanted = Backends::VULKAN | Backends::DX12 | Backends::METAL;
    let skip = Backends::DX12 | Backends::METAL;
    let full = full_report(rtx(Backend::Vulkan));
    // The honest skip passes, and so does the honest full enumeration on an iGPU-only box.
    skip_guard(&full, &full[..1], skip, wanted).expect("the honest skip");
    let igpu = facts(
        "Intel UHD 770",
        Backend::Vulkan,
        DeviceType::IntegratedGpu,
        (0x8086, 0x4680),
        VK13,
    );
    let igpu_full = full_report(igpu);
    skip_guard(&igpu_full, &igpu_full, Backends::empty(), wanted).expect("the honest full run");
    // 1. A skip without a qualifying Vulkan adapter (the iGPU box skipping D3D12 and WARP).
    let e = skip_guard(&igpu_full, &igpu_full[..1], skip, wanted).expect_err("skip on an iGPU");
    assert!(e.contains("no discrete adapter above the floor"), "{e}");
    // 2. A missed skip: the RTX 3080 box enumerating D3D12 in GPU mode Single.
    let e = skip_guard(&full, &full, Backends::empty(), wanted).expect_err("missed skip");
    assert!(e.contains("enumerated anyway"), "{e}");
    // 3. An enumerated adapter lost from the report.
    let e = skip_guard(&igpu_full, &igpu_full[1..], Backends::empty(), wanted)
        .expect_err("a lost adapter");
    assert!(e.contains("does not list every enumerated adapter"), "{e}");
    // 4. A row of a skipped backend.
    let e = skip_guard(&full, &full[..2], skip, wanted).expect_err("a skipped backend's row");
    assert!(e.contains("was skipped"), "{e}");
}

#[test]
fn this_machine_single_skips_d3d12_exactly_when_vulkan_suffices() {
    let Some(full) = common::pool(&PoolOptions {
        mode: GpuMode::Multi,
        ..PoolOptions::default()
    }) else {
        return;
    };
    let full_report = full.report().to_vec();
    assert!(
        full.skipped_backends().is_empty(),
        "GPU mode Multi skipped a backend"
    );
    drop(full);
    let Some(single) = common::pool(&PoolOptions::default()) else {
        return;
    };
    for r in single.report() {
        println!(
            "single: {:<60} {:?} -> {}",
            r.facts.label(),
            r.facts.backend,
            r.verdict.reason()
        );
    }
    println!("single: skipped {:?}", single.skipped_backends());
    // The backends the pool asks for: the default set, or WGPU_BACKEND's.
    let wanted =
        Backends::from_env().unwrap_or(Backends::VULKAN | Backends::DX12 | Backends::METAL);
    skip_guard(
        &full_report,
        single.report(),
        single.skipped_backends(),
        wanted,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // The skip never changes which GPU the user gets.
    let primary = full_report
        .iter()
        .find(|r| r.verdict == AdapterVerdict::Selected(0))
        .map(|r| r.facts.label());
    assert_eq!(primary, Some(single.primary().label()));
}

#[test]
fn the_dx12_probe_can_say_no() {
    let Ok(pool) = AdapterPool::new(&PoolOptions {
        backends: Some(forge_gpu::wgpu::Backends::DX12),
        software: SoftwarePolicy::Include,
        mode: GpuMode::Multi,
        ..PoolOptions::default()
    }) else {
        println!("AWAITING(no Direct3D 12 adapter on this machine)");
        return;
    };
    for d in pool.devices() {
        let fl12 = forge_gpu::probe::dx12_supports_level(&d.adapter, forge_gpu::probe::D3D_FL_12_0);
        assert_eq!(fl12, Some(true), "{}", d.label());
        // 0xd000 ("13_0") does not exist: the probe must be able to answer false.
        let bogus = forge_gpu::probe::dx12_supports_level(&d.adapter, 0xd000);
        assert_eq!(bogus, Some(false), "{}", d.label());
    }
}

#[test]
fn no_adapter_is_awaiting_locally() {
    let r = common::no_adapter_outcome("GPU-0001: no graphics adapter found", false);
    assert!(r.is_ok_and(|m| m.contains("no GPU adapter")));
}

/// W9 for "a GPU guard never passes silently": under `CI=true` no adapter is a failure.
#[test]
fn positive_control_no_adapter_under_ci_fails() {
    let r = common::no_adapter_outcome("GPU-0001: no graphics adapter found", true);
    assert!(r.is_err_and(|m| m.contains("CI=true")));
}

/// A test build's pool holds this process's shared GPU turn while it lives
/// (`test-gpu-turn`, WP-19), so no timed body measures beside its GPU work; dropping the pool
/// gives the turn back. (This binary's other tests may hold pools of their own, so the check
/// is on the lock file from another handle only while this pool is known to be alive.)
#[test]
fn a_test_pool_holds_a_gpu_turn_while_it_lives() {
    let Some(pool) = common::pool(&PoolOptions::default()) else {
        return;
    };
    assert!(
        forge_trace::timed::holds_gpu_turn(),
        "a pool built with test-gpu-turn holds no GPU turn: timed bodies would measure beside it"
    );
    let other = std::fs::OpenOptions::new()
        .write(true)
        .open(std::env::temp_dir().join(forge_trace::timed::LOCK_FILE))
        .expect("the lock file exists while a turn is held");
    assert!(
        matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        "the timed lock was free for exclusive use while a pool lived"
    );
    drop(pool);
}
