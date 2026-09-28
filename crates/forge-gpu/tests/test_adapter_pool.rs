//! `test_adapter_pool` — the adapter pool and the O-8 floor (Ch.9, M1-1).
//!
//! * The floor refuses every class of adapter O-8 excludes, each with a user-facing reason.
//! * Selection counts one physical GPU once across backends, keeps two identical cards as
//!   two, orders discrete before integrated before software, applies the software policy
//!   and the device cap — on synthetic machines (a mismatched second card included).
//! * On this machine: every adapter the driver lists appears in the report with a verdict,
//!   the probes read real API levels, and the primary is a hardware adapter above the floor.
//!
//! Positive control (W2): `positive_control_every_excluded_class_is_refused` feeds one
//! adapter of each excluded class and asserts each is refused with its reason; the real
//! D3D12 probe is shown able to answer "no" (`the_dx12_probe_can_say_no`).

mod common;

use forge_gpu::floor::{FLOOR_TEXT, below_floor_message};
use forge_gpu::wgpu::{Backend, DeviceType};
use forge_gpu::{
    AdapterFacts, AdapterPool, AdapterVerdict, ApiLevel, Choice, GpuMode, PoolOptions,
    SoftwarePolicy, check_floor, plan_selection,
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
