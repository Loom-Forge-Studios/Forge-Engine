//! `test_perf_gate` — the named performance budgets and the regression gate (Ch.29, DoD
//! M1-11; gate rows `C-perf-gate`, `C-perf-gate-gpu-timestamps`).
//!
//! `tests/perf/budgets.ron` names every budget of the base edition: the atmosphere's table
//! precomputation (`render.atmosphere.precompute`, the GPU time of its dispatches from their
//! timestamps) and the 2D render path (WP-U15, Ch.35, M4-11): the GPU time of each of its four
//! passes and their sum (`render2d.frame.gpu.*`, the perf frame of
//! `forge_2d::scenes::perf_scene` at 1920x1080), its last mile on the CPU
//! (`render2d.frame.prepare_cpu`), its draw calls and graph rebuilds (counters), one step of
//! the 2D solver with 2,000 bodies (`phys2d.step`) and the 2D cold start (`2d.cold_start`,
//! Ch.31 §31.5), and one 3D physics step of 2,000 bodies per backend (`phys.step.avian3d`,
//! `phys.step.rapier3d`; WP-60). This test measures all of them on the machine it runs on and fails when any
//! is over its budget; the gate itself (bands, slack floors, the SLACK check, coverage both
//! ways) is `forge_perf_gate::check`. The measured table is written to
//! `$CARGO_TARGET_DIR/perf-gate.txt`.
//!
//! Every measuring test runs alone (`forge_perf_gate::measuring`: a child process at the High
//! priority class holding the machine-wide timed lock, WP-19); the GPU is measured by
//! timestamps. Scheduling, not a tolerance (W5).
//!
//! Positive controls (W2): `positive_control_an_injected_gpu_regression_fails_the_gate` — the
//! 2D frame with every light evaluated 24 more times per pixel fails
//! `render2d.frame.gpu.lights` against the clean run's own numbers, so it bites on every
//! device; `positive_control_a_slower_2d_light_pass_fails_its_row` — a few more evaluations,
//! sized per class, fail the row against the committed baseline;
//! `positive_control_a_1_5x_2d_pass_fails_its_own_row` — each 2D pass made 1.5x fails
//! exactly its own row; `positive_control_a_blanket_slack_is_refused` — the gate refuses a
//! slack floor or a band under which a 1.5x pass could pass;
//! `positive_control_an_injected_2d_cpu_regression_fails_the_gate` — the solver's velocity
//! iterations run nine times over fail `phys2d.step`;
//! `positive_control_an_injected_3d_physics_regression_fails_the_gate` — every backend's step
//! run sixteen times over fails its own `phys.step.<backend>` row;
//! `positive_control_a_slower_precompute_fails_its_row` — the atmosphere tables built with
//! eight scattering orders instead of four fail `render.atmosphere.precompute` against the
//! committed baseline (its timestamps see the precomputation's own work);
//! `positive_control_unbatched_sprites_fail_the_draw_call_row`; and
//! `positive_control_coverage_is_checked_both_ways`.

use std::collections::BTreeMap;

use forge_perf_gate::{
    Budgets, Kind, at_baseline, calibrate, check, device_class, measure_2d, measure_2d_cpu,
    measure_atmosphere, measure_phys_cpu, pool, report, tables,
};
use forge_render::AtmosphereSettings;

const ROW: &str = "C-perf-gate";

fn budgets() -> Budgets {
    Budgets::base().unwrap_or_else(|e| panic!("{e}"))
}

/// The 2D rows of the committed budget file only.
fn budgets_2d() -> Budgets {
    budgets().only(&["render2d.", "phys2d.", "2d."])
}

#[test]
fn the_named_budgets_hold() {
    forge_perf_gate::measuring(|| {
        let Some(pool) = pool(ROW)? else {
            return Ok(());
        };
        let dev = pool.primary();
        let class = device_class(dev);
        let calib = calibrate();
        let mut m = BTreeMap::new();
        measure_atmosphere(dev, &tables(dev), 3, &mut m)?;
        measure_2d(dev, forge_2d::Faults2d::default(), &mut m, calib)?;
        measure_2d_cpu(forge_2d::Faults2d::default(), &mut m, calib)?;
        measure_phys_cpu(forge_phys::PhysFaults::default(), &mut m, calib)?;
        let text = report(&m, class, calib, &dev.label(), "perf-gate.txt");
        println!("{text}");
        let v = check(&budgets(), &m, class);
        for l in &v.lines {
            println!("{l}");
        }
        for n in &v.not_gated {
            println!("not gated: {n}");
        }
        assert!(v.violations.is_empty(), "{:#?}", v.violations);
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_injected_gpu_regression_fails_the_gate() {
    forge_perf_gate::measuring(|| {
        let Some(pool) = pool(ROW)? else {
            return Ok(());
        };
        let dev = pool.primary();
        let calib = calibrate();
        let mut clean = BTreeMap::new();
        measure_2d(dev, forge_2d::Faults2d::default(), &mut clean, calib)?;
        let mut faulted = BTreeMap::new();
        measure_2d(
            dev,
            forge_2d::Faults2d {
                light_repeats: 24,
                ..forge_2d::Faults2d::default()
            },
            &mut faulted,
            calib,
        )?;
        // Budgets baselined on this device's own clean run: the gate must still see the fault.
        // No slack: the clean run is its own baseline, so it passes by construction.
        let b = budgets_2d();
        let own = Budgets {
            gpu_slack_ms: BTreeMap::from([("self".to_string(), 0.0)]),
            rows: b
                .rows
                .iter()
                .filter(|r| clean.contains_key(&r.name))
                .map(|r| {
                    let mut r = r.clone();
                    if r.kind == Kind::GpuMs {
                        r.baseline = BTreeMap::from([("self".to_string(), clean[&r.name])]);
                    }
                    if r.kind == Kind::CpuRatio {
                        r.ratio = Some(clean[&r.name]);
                    }
                    r
                })
                .collect(),
            ..b
        };
        let clean_verdict = check(&own, &clean, Some("self"));
        assert!(
            clean_verdict.violations.is_empty(),
            "the clean run passes its own baseline: {:#?}",
            clean_verdict.violations
        );
        let v = check(&own, &faulted, Some("self"));
        let lights = "render2d.frame.gpu.lights";
        println!(
            "lights clean {:.3} ms, faulted {:.3} ms",
            clean[lights], faulted[lights]
        );
        assert!(
            v.violations.iter().any(|x| x.contains(lights)),
            "lights evaluated 25x must fail the gate: {:#?}",
            v.violations
        );
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

/// W2 for the SLACK check: a 0.05 ms slack on the RTX 3080's 0.02-0.07 ms 2D passes is refused
/// (it would let the upscale pass grow ~3.8x before its own row failed); so is WP-11's
/// relative band of 0.5 (a pass 15 % under its baseline could grow 1.5x unseen), for every GPU
/// row of every class, and a 2 ms WARP floor on its ~6.6 ms passes; the committed file passes.
#[test]
fn positive_control_a_blanket_slack_is_refused() {
    let b = budgets();
    let at = at_baseline(&b, "rtx3080");
    let ok = check(&b, &at, Some("rtx3080"));
    assert!(
        !ok.violations.iter().any(|x| x.starts_with("SLACK")),
        "{:#?}",
        ok.violations
    );
    let mut blanket = b.clone();
    blanket.gpu_slack_ms.insert("rtx3080".into(), 0.05);
    let v = check(&blanket, &at, Some("rtx3080"));
    assert!(
        v.violations
            .iter()
            .any(|x| x.starts_with("SLACK render2d.frame.gpu.upscale")),
        "a 0.05 ms slack on 0.02 ms passes must be refused: {:#?}",
        v.violations
    );
    let mut wide = b.clone();
    wide.gpu_band = 0.5;
    let v = check(&wide, &at, Some("rtx3080"));
    let refused = v
        .violations
        .iter()
        .filter(|x| x.starts_with("SLACK"))
        .count();
    let gpu_rows: usize = b
        .rows
        .iter()
        .filter(|r| r.kind == Kind::GpuMs)
        .map(|r| r.baseline.len())
        .sum();
    assert!(gpu_rows > 0, "the base file has GPU rows");
    assert_eq!(
        refused, gpu_rows,
        "a 0.5 band must be refused for every GPU row of every class: {:#?}",
        v.violations
    );
    let mut floor = b.clone();
    floor.gpu_slack_ms.insert("warp".into(), 2.0);
    let v = check(&floor, &at, Some("rtx3080"));
    assert!(
        v.violations
            .iter()
            .any(|x| x.starts_with("SLACK render2d.frame.gpu.composite") && x.contains("warp")),
        "a 2 ms WARP floor on a 6.6 ms pass must be refused: {:#?}",
        v.violations
    );
}

/// W2 for the atmosphere row measured from timestamps: the same tables built with eight
/// scattering orders instead of four fail `render.atmosphere.precompute` against the
/// committed baseline of this device's class — the timestamps see the precomputation's own
/// work grow, so excluding other processes' GPU work did not make the row blind.
#[test]
fn positive_control_a_slower_precompute_fails_its_row() {
    forge_perf_gate::measuring(|| {
        let Some(pool) = pool(ROW)? else {
            return Ok(());
        };
        let dev = pool.primary();
        let Some(class) = device_class(dev) else {
            println!(
                "{ROW}: atmosphere control NOT APPLICABLE on {}: no committed GPU baseline",
                dev.label()
            );
            return Ok(());
        };
        let slow = AtmosphereSettings {
            orders: 8,
            ..tables(dev)
        };
        let mut m = BTreeMap::new();
        measure_atmosphere(dev, &slow, 1, &mut m)?;
        let only = budgets().only(&["render.atmosphere."]);
        let v = check(&only, &m, Some(class));
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("REGRESSION render.atmosphere.precompute ")),
            "eight scattering orders must fail the precompute row on {class}: {:#?} ({m:?})",
            v.violations
        );
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_coverage_is_checked_both_ways() {
    let b = budgets();
    // Everything measured at exactly its budget except one row missing and one extra.
    let mut m = at_baseline(&b, "rtx3080");
    assert!(check(&b, &m, Some("rtx3080")).violations.is_empty());
    let first = b.rows[0].name.clone();
    m.remove(&first);
    m.insert("render2d.frame.gpu.new_pass".into(), 0.1);
    let v = check(&b, &m, Some("rtx3080"));
    assert!(
        v.violations
            .iter()
            .any(|x| x.contains(&first) && x.contains("not measured"))
    );
    assert!(
        v.violations
            .iter()
            .any(|x| x.contains("new_pass") && x.contains("no named budget"))
    );
}

/// The 2D light pass bites at its band: every light evaluated a few more times per pixel
/// (`Faults2d::light_repeats`, sized per class to land well past the band) fails
/// `render2d.frame.gpu.lights` against the committed baseline of the device's class. RTX
/// 3080: six more (0.066 -> ~0.110 ms, ~1.66x; the pass is raster-bound, so three more add
/// only ~12 % and twenty-four ~4.9x); WARP: three more (13.8 -> ~28 ms, ~2x).
#[test]
fn positive_control_a_slower_2d_light_pass_fails_its_row() {
    forge_perf_gate::measuring(|| {
        let Some(pool) = pool(ROW)? else {
            return Ok(());
        };
        let dev = pool.primary();
        let class = device_class(dev);
        let repeats = match class {
            Some("rtx3080") => 6,
            Some("warp") => 3,
            _ => {
                println!(
                    "{ROW}: 2D light control NOT APPLICABLE on {} (class {class:?}): no committed GPU baseline",
                    dev.label()
                );
                return Ok(());
            }
        };
        let calib = calibrate();
        let mut m = BTreeMap::new();
        measure_2d(
            dev,
            forge_2d::Faults2d {
                light_repeats: repeats,
                ..forge_2d::Faults2d::default()
            },
            &mut m,
            calib,
        )?;
        let lights = "render2d.frame.gpu.lights";
        println!(
            "2d lights with {repeats} extra evaluations: {:.4} ms",
            m[lights]
        );
        let v = check(&budgets_2d(), &m, class);
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("REGRESSION") && x.contains(lights)),
            "a slower light pass must fail its own committed row: {:#?}",
            v.violations
        );
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

/// Every 2D GPU row catches a 1.5x regression of its own pass on this device.
#[test]
fn positive_control_a_1_5x_2d_pass_fails_its_own_row() {
    forge_perf_gate::measuring(|| {
        let Some(pool) = pool(ROW)? else {
            return Ok(());
        };
        let dev = pool.primary();
        let Some(class) = device_class(dev) else {
            println!(
                "{ROW}: 2D 1.5x control NOT APPLICABLE on {}: no committed GPU baseline",
                dev.label()
            );
            return Ok(());
        };
        let calib = calibrate();
        let b = budgets_2d();
        let mut clean = BTreeMap::new();
        measure_2d(dev, forge_2d::Faults2d::default(), &mut clean, calib)?;
        let total = "render2d.frame.gpu.total";
        let passes: Vec<String> = clean
            .keys()
            .filter(|k| k.starts_with("render2d.frame.gpu.") && k.as_str() != total)
            .cloned()
            .collect();
        assert_eq!(passes.len(), 4, "the four 2D passes are timed: {passes:?}");
        for pass in &passes {
            let mut m = clean.clone();
            let grown = m[pass] * 0.5;
            *m.entry(pass.clone()).or_default() += grown;
            *m.entry(total.to_string()).or_default() += grown;
            let v = check(&b, &m, Some(class));
            println!("{pass}: {:.4} ms -> {:.4} ms", clean[pass], m[pass]);
            assert!(
                v.violations
                    .iter()
                    .any(|x| x.starts_with("REGRESSION") && x.contains(pass.as_str())),
                "a 1.5x {pass} must fail its own row on {class}: {:#?}",
                v.violations
            );
        }
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

/// The 2D solver's row bites: a solver running its velocity iterations nine times over
/// (`Faults2d::solver_repeats` = 8) fails `phys2d.step`. Sized to land well past the band on
/// every leg: the step's ratio grows from ~0.10-0.15 to 0.32-0.36 (allowed 0.170) on the dev
/// box and in the lavapipe container (2026-09-24).
#[test]
fn positive_control_an_injected_2d_cpu_regression_fails_the_gate() {
    forge_perf_gate::measuring(|| {
        let calib = calibrate();
        let mut m = BTreeMap::new();
        measure_2d_cpu(
            forge_2d::Faults2d {
                solver_repeats: 8,
                ..forge_2d::Faults2d::default()
            },
            &mut m,
            calib,
        )?;
        let only = budgets().only(&["phys2d.step"]);
        let v = check(&only, &m, None);
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("REGRESSION") && x.contains("phys2d.step")),
            "a solver nine times over must fail the solver's row: {:#?} ({m:?})",
            v.violations
        );
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

/// The 3D physics rows bite: every backend's step run sixteen times over
/// (`PhysFaults::step_repeats` = 15) fails its own `phys.step.<backend>` row. Sized for the
/// provisional allowance (0.85 x 1.5): the cheaper backend, rapier3d, measures ~0.12 clean
/// on a 4-vCPU cloud machine, so sixteen steps land well past it; once the rows are
/// baselined on the dev box a smaller fault would do. CPU only: it needs no adapter.
#[test]
fn positive_control_an_injected_3d_physics_regression_fails_the_gate() {
    forge_perf_gate::measuring(|| {
        let calib = calibrate();
        let mut m = BTreeMap::new();
        measure_phys_cpu(
            forge_phys::PhysFaults {
                step_repeats: 15,
                ..forge_phys::PhysFaults::default()
            },
            &mut m,
            calib,
        )?;
        let v = check(&budgets().only(&["phys.step."]), &m, None);
        for b in forge_phys::FIRST_PARTY {
            let row = format!("phys.step.{b}");
            assert!(
                v.violations
                    .iter()
                    .any(|x| x.starts_with("REGRESSION") && x.contains(&row)),
                "a step sixteen times over must fail {row}: {:#?} ({m:?})",
                v.violations
            );
        }
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

/// The draw-call counter bites: sprites drawn one call each fail
/// `render2d.frame.draw_calls`.
#[test]
fn positive_control_unbatched_sprites_fail_the_draw_call_row() {
    forge_perf_gate::measuring(|| {
        let Some(pool) = pool(ROW)? else {
            return Ok(());
        };
        let dev = pool.primary();
        let calib = calibrate();
        let mut m = BTreeMap::new();
        measure_2d(
            dev,
            forge_2d::Faults2d {
                no_batching: true,
                ..forge_2d::Faults2d::default()
            },
            &mut m,
            calib,
        )?;
        let only = budgets().only(&["render2d.frame.draw_calls"]);
        let v = check(&only, &m, device_class(dev));
        assert!(
            v.violations
                .iter()
                .any(|x| x.contains("render2d.frame.draw_calls")),
            "unbatched sprites must fail the draw-call row: {:#?}",
            v.violations
        );
        Ok(())
    })
    .unwrap_or_else(|e| panic!("{e}"));
}
