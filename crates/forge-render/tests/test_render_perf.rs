// timed-gates: exempt(the CPU prepare time is printed; pass times come from GPU timestamps, not the wall clock)
//! `test_render_perf` — per-pass GPU timings of a representative frame (Ch.10 §10.9).
//!
//! 1920x1080; a floor, 2,500 instances (cubes and spheres) on the surface of a 6,371 km ground sphere,
//! 256 clustered point lights, the sun with four 2048² cascades (`common::perf_scene`).
//! Three warm-up frames, then 20 timed frames: every render-graph pass reports its GPU time
//! (encoder timestamps) and CPU recording time, and preparation (the f64 last mile, culling,
//! clustering, cascade fit) is timed on the CPU. The medians are printed and written to
//! `$CARGO_TARGET_DIR/forge-render-perf.txt`; Ch.10 §10.9 records them. Asserted: every
//! pass is timed on a device with timestamps, a steady-state frame allocates no GPU memory,
//! and it reuses the compiled frame graph. Positive control (W2):
//! `positive_control_missing_timings_are_caught`.
//! (Budgets and the regression gate are DoD M1-11.)

mod common;

use std::collections::BTreeMap;
use std::time::Instant;

use forge_frames::{DVec3, FramePos, Tick};
use forge_render::{Camera, Instance, Material, MeshData, RenderOptions, Renderer, Scene};

const ROW: &str = "C-render-pass-timings";

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
fn per_pass_gpu_timings_are_recorded() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let (w, h) = (1920, 1080);
    let mut opts = RenderOptions::new(w, h);
    opts.timing = true;
    let mut r = Renderer::new(dev, opts).expect("renderer");
    let (scene, cam) = common::perf_scene(&mut r, root);
    let target = common::target(dev, w, h);
    let mut gpu: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut cpu: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut prep = Vec::new();
    let mut last = None;
    for frame in 0..23 {
        let t0 = Instant::now();
        let f = r.prepare(&scene, &cam, &tree, Tick(0)).expect("prepare");
        let prep_ms = t0.elapsed().as_secs_f64() * 1e3;
        let rep = r.render(dev, &f, &target).expect("render");
        // Hand the buffers back, as render_scene does: steady-state preparation reuses them.
        r.recycle(f);
        if frame < 3 {
            continue;
        }
        assert_eq!(
            rep.exec.allocations, 0,
            "frame {frame}: a steady-state frame allocates nothing"
        );
        assert!(
            !rep.graph_rebuilt,
            "frame {frame}: an unchanged topology reuses the compiled graph"
        );
        prep.push(prep_ms);
        if let Err(e) = timings_complete(&rep.passes, r.gpu_timing_supported()) {
            panic!("frame {frame}: {e}");
        }
        for p in &rep.passes {
            if let Some(g) = p.gpu_ns {
                gpu.entry(p.name.clone()).or_default().push(g / 1e6);
            }
            cpu.entry(p.name.clone())
                .or_default()
                .push(p.cpu_record_ns as f64 / 1e6);
        }
        last = Some(rep);
    }
    let rep = last.expect("timed frames");
    assert!(
        rep.passes.len() >= 7,
        "cascades, sky, shells and tonemap are all timed: {:?}",
        rep.passes.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    let mut out = format!(
        "forge-render frame, {w}x{h}, {} instances ({} uploaded, {} draw calls), {} lights ({} cluster entries), {} cascades — {}\n",
        rep.prepare.instances,
        rep.prepare.uploaded,
        rep.prepare.draw_calls,
        rep.prepare.lights,
        rep.prepare.cluster_entries,
        4,
        dev.label()
    );
    out += &format!("prepare (CPU, median of 20): {:.3} ms\n", median(prep));
    let mut total = 0.0;
    for p in &rep.passes {
        let g = gpu.get(&p.name).map(|v| median(v.clone()));
        let c = median(cpu[&p.name].clone());
        total += g.unwrap_or(0.0);
        out += &format!(
            "{:<18} gpu {:>8} ms   cpu record {:.3} ms\n",
            p.name,
            g.map_or("n/a".to_string(), |g| format!("{g:.3}")),
            c
        );
    }
    out += &format!("sum of pass GPU medians: {total:.3} ms\n");
    println!("{out}");
    let path = common::target_file("forge-render-perf.txt");
    std::fs::write(&path, &out).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// What `per_pass_gpu_timings_are_recorded` requires of a frame's timings: every pass named,
/// and every pass GPU-timed when the device has timestamps.
fn timings_complete(passes: &[forge_gpu::PassTiming], gpu: bool) -> Result<(), String> {
    if passes.is_empty() {
        return Err("no pass timings".into());
    }
    if gpu && let Some(p) = passes.iter().find(|p| p.gpu_ns.is_none()) {
        return Err(format!("pass {} has no GPU time", p.name));
    }
    Ok(())
}

#[test]
fn positive_control_missing_timings_are_caught() {
    let t = |name: &str, gpu_ns| forge_gpu::PassTiming {
        name: name.into(),
        cpu_record_ns: 1,
        gpu_ns,
    };
    assert!(timings_complete(&[t("a", Some(1.0)), t("b", Some(2.0))], true).is_ok());
    assert!(timings_complete(&[t("a", Some(1.0)), t("b", None)], true).is_err());
    assert!(timings_complete(&[], false).is_err());
    // An untimed frame really reports nothing, so the timed path is what fills the table.
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let (tree, root) = common::tree();
    let mut r = Renderer::new(dev, RenderOptions::new(64, 64)).expect("renderer");
    let cube = r.add_mesh(&MeshData::cube()).expect("cube");
    let m = r.add_material(Material::default());
    let scene = Scene {
        instances: vec![Instance::new(
            FramePos::new(root, DVec3::new(0.0, 0.0, -3.0)),
            cube,
            m,
        )],
        ..Scene::default()
    };
    let cam = Camera::looking(
        FramePos::origin_of(root),
        DVec3::new(0.0, 0.0, -1.0),
        DVec3::Y,
        1.0,
    )
    .expect("camera");
    let target = common::target(dev, 64, 64);
    let rep = r
        .render_scene(dev, &scene, &cam, &tree, Tick(0), &target)
        .expect("render");
    assert!(timings_complete(&rep.passes, false).is_err());
}
