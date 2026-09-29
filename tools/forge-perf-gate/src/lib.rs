//! `forge-perf-gate` — the named performance budgets and their regression gate (Ch.29, DoD
//! M1-11): the budget file's model, the gate itself ([`check`]) and the measurements every
//! edition shares (the atmosphere's table precomputation, the 2D render path and the 2D CPU
//! rows). The base edition's gate is `tests/test_perf_gate.rs` beside this crate, over
//! `tests/perf/budgets.ron`; an edition with more to measure runs the same gate over its own
//! budget file with its own measurements (ADR 0062).
//!
//! * **GPU times** have a baseline per device class (`rtx3080` — the dev box; `warp` —
//!   Microsoft's software rasteriser, what the GPU-less Windows CI leg renders with).
//!   Allowed: `baseline x (1 + gpu_band)`, or `baseline + gpu_slack_ms[class]` where the
//!   relative band is below that class's timestamp quantisation and run-to-run spread. **Every
//!   row catches a `catch`x regression of its own pass** (WP-18): the gate refuses a budget
//!   file in which a pass `drift` under its baseline could grow `catch` times unseen (the SLACK
//!   check). A device of another class has no GPU baseline: its GPU rows are printed as *not
//!   gated* (never silently passed), and its counters and CPU rows are still gated.
//! * **CPU times** are a ratio to a calibration workload measured in the same process
//!   (integer-lattice fBm), so one number holds on a laptop, the dev box and a CI runner.
//! * **Counters** are exact maxima.
//! * **Coverage both ways**: a budget row nobody measured, and a measurement with no budget
//!   row, are failures.
//!
//! **The conditions** (WP-19, ADR 0028): every measuring test runs its body alone through
//! [`measuring`] (`forge_trace::timed::run_timed_alone`: a child process, the High priority
//! class, the machine-wide timed lock), and the GPU is measured by timestamps, never by the
//! wall clock. Scheduling, not a tolerance (W5): no row, band or floor is widened for load.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use forge_gpu::{AdapterPool, GpuDevice, GpuTimer, PoolOptions, wgpu};
use forge_num::{IVec3, fbm_i};
use forge_render::{AtmosphereSettings, GpuAtmosphere};
use serde::Deserialize;

/// The base budget file, relative to the repository root.
pub const BASE_BUDGETS: &str = "tests/perf/budgets.ron";

/// What a row measures.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Kind {
    /// GPU milliseconds, baselined per device class.
    GpuMs,
    /// CPU time over the calibration workload's.
    CpuRatio,
    /// An exact maximum.
    Counter,
}

/// One named budget.
#[derive(Clone, Debug, Deserialize)]
pub struct Row {
    pub name: String,
    pub kind: Kind,
    /// GpuMs: device class -> milliseconds.
    #[serde(default)]
    pub baseline: BTreeMap<String, f64>,
    /// CpuRatio: measured / calibration.
    #[serde(default)]
    pub ratio: Option<f64>,
    /// Counter: the maximum.
    #[serde(default)]
    pub max: Option<f64>,
    /// Why this budget exists.
    pub why: String,
}

/// A budget file.
#[derive(Clone, Debug, Deserialize)]
pub struct Budgets {
    /// CpuRatio rows: the relative band.
    pub band: f64,
    /// GpuMs rows: the relative band.
    pub gpu_band: f64,
    /// GpuMs rows must catch a regression of this factor ...
    pub catch: f64,
    /// ... of a pass measuring this fraction below its baseline.
    pub drift: f64,
    /// GpuMs: device class -> the absolute allowance below which the band does not reach
    /// (that class's timestamp quantisation and run-to-run spread of a pass median).
    pub gpu_slack_ms: BTreeMap<String, f64>,
    pub cpu_slack: f64,
    pub rows: Vec<Row>,
}

impl Budgets {
    /// The budget file at `path`.
    pub fn at(path: &Path) -> Result<Budgets, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The base edition's budget file ([`BASE_BUDGETS`]).
    pub fn base() -> Result<Budgets, String> {
        Self::at(&repo_root().join(BASE_BUDGETS))
    }

    /// These budgets with only the rows whose names start with one of `prefixes`.
    #[must_use]
    pub fn only(&self, prefixes: &[&str]) -> Budgets {
        Budgets {
            rows: self
                .rows
                .iter()
                .filter(|r| prefixes.iter().any(|p| r.name.starts_with(p)))
                .cloned()
                .collect(),
            ..self.clone()
        }
    }
}

/// The repository root (this crate is `tools/forge-perf-gate`).
#[must_use]
pub fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Run a measuring test's body alone (see the crate docs): everything that creates a
/// device, calibrates or times goes in `body`, which runs only in the child process. An
/// `assert!` failing in the body, or an error it returns, comes back as an `Err` carrying
/// the child's output, for the test to fail with.
pub fn measuring(body: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let s = forge_trace::timed::run_timed_alone(|| body().map(|()| String::new()))?;
    if !s.is_empty() {
        println!("{s}");
    }
    Ok(())
}

/// What the gate found.
#[derive(Debug, Default)]
pub struct Verdict {
    pub violations: Vec<String>,
    pub not_gated: Vec<String>,
    pub lines: Vec<String>,
}

/// A GPU row's allowance: the relative band, or the class's slack floor where that is larger.
#[must_use]
pub fn gpu_allowed(b: &Budgets, base: f64, slack: f64) -> f64 {
    (base * (1.0 + b.gpu_band)).max(base + slack)
}

/// The gate: every row against its measurement, both directions of coverage.
#[must_use]
pub fn check(b: &Budgets, measured: &BTreeMap<String, f64>, class: Option<&str>) -> Verdict {
    let mut v = Verdict::default();
    // Every GPU row, of every class in the file, must catch a `catch`x regression of a pass
    // that measures `drift` below its baseline: its allowance (the relative band or the
    // class's slack floor, whichever is larger) must stay under `catch x (1 - drift)`.
    let limit = b.catch * (1.0 - b.drift);
    for row in b.rows.iter().filter(|r| r.kind == Kind::GpuMs) {
        for (c, base) in &row.baseline {
            let slack = b.gpu_slack_ms.get(c).copied().unwrap_or(0.0);
            let allowed = gpu_allowed(b, *base, slack) / base;
            if allowed >= limit {
                v.violations.push(format!(
                    "SLACK {}: class {c:?} allows {allowed:.3}x its baseline (gpu_band {}, \
                     gpu_slack_ms {slack} ms on {base} ms): a {}x pass {:.0} % under its \
                     baseline would not fail (limit {limit:.3}x)",
                    row.name,
                    b.gpu_band,
                    b.catch,
                    b.drift * 100.0
                ));
            }
        }
    }
    for row in &b.rows {
        let Some(&m) = measured.get(&row.name) else {
            v.violations
                .push(format!("{}: budgeted but not measured", row.name));
            continue;
        };
        let (allowed, what) = match row.kind {
            Kind::GpuMs => {
                let Some((base, slack)) =
                    class.and_then(|c| Some((*row.baseline.get(c)?, *b.gpu_slack_ms.get(c)?)))
                else {
                    v.not_gated.push(format!(
                        "{}: {m:.3} ms (no baseline or slack for device class {:?})",
                        row.name, class
                    ));
                    continue;
                };
                (
                    gpu_allowed(b, base, slack),
                    format!("baseline {base:.3} ms"),
                )
            }
            Kind::CpuRatio => {
                let Some(r) = row.ratio else {
                    v.violations
                        .push(format!("{}: CpuRatio row without a ratio", row.name));
                    continue;
                };
                (r * (1.0 + b.band) + b.cpu_slack, format!("ratio {r:.4}"))
            }
            Kind::Counter => {
                let Some(max) = row.max else {
                    v.violations
                        .push(format!("{}: Counter row without a max", row.name));
                    continue;
                };
                (max, format!("max {max}"))
            }
        };
        let line = format!(
            "{:40} {m:12.4}   allowed {allowed:10.4}   ({what})",
            row.name
        );
        if m > allowed {
            v.violations.push(format!("REGRESSION {line}"));
        }
        v.lines.push(line);
    }
    for name in measured.keys() {
        if !b.rows.iter().any(|r| &r.name == name) {
            v.violations
                .push(format!("{name}: measured but has no named budget"));
        }
    }
    v
}

/// The median of `v` (not empty).
#[must_use]
pub fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The calibration workload (ms): 20,000 lattice fBm evaluations of 15 octaves, median of 7.
#[must_use]
pub fn calibrate() -> f64 {
    let mut runs = Vec::new();
    for r in 0..7 {
        let t0 = Instant::now();
        let mut acc = 0.0;
        for i in 0..20_000i64 {
            acc += fbm_i(IVec3::new(i * 7919, i * 104_729 + r, -i * 31), 20, 15, 99);
        }
        std::hint::black_box(acc);
        runs.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    median(runs)
}

/// The device class GPU baselines are recorded for: the dev box's GPU, and Microsoft WARP.
/// Other software rasterisers (lavapipe, llvmpipe) differ from WARP by integer factors, so
/// they are their own class, not yet baselined (gate row C-perf-gate-linux-leg).
#[must_use]
pub fn device_class(dev: &GpuDevice) -> Option<&'static str> {
    if dev.facts.name.contains("Microsoft Basic Render") {
        Some("warp")
    } else if dev.facts.name.contains("RTX 3080") {
        Some("rtx3080")
    } else {
        None
    }
}

/// Whether this is a CI run.
#[must_use]
pub fn on_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// The adapter pool the gate measures on (`None`: no adapter, recorded as an observation
/// for gate row `row`; on CI that fails instead).
pub fn pool(row: &str) -> Result<Option<AdapterPool>, String> {
    let obs = repo_root()
        .join("target/gate-observations")
        .join(format!("{row}.txt"));
    let software = if std::env::var_os("FORGE_RENDER_SOFTWARE").is_some_and(|v| v == "1") {
        forge_gpu::SoftwarePolicy::Only
    } else {
        forge_gpu::SoftwarePolicy::FallbackOnly
    };
    match AdapterPool::new(&PoolOptions {
        wanted_features: GpuTimer::FEATURES,
        max_devices: Some(1),
        software,
        ..PoolOptions::default()
    }) {
        Ok(p) => {
            let _ = std::fs::remove_file(&obs);
            Ok(Some(p))
        }
        Err(e) => {
            if on_ci() {
                return Err(format!(
                    "CI=true and no GPU adapter ({e}): the perf gate must run"
                ));
            }
            println!("{row}: AWAITING(no GPU adapter: {e})");
            if let Some(d) = obs.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&obs, format!("no GPU adapter: {e}"));
            Ok(None)
        }
    }
}

/// The atmosphere tables the gate builds: the real resolution on a GPU, the quarter-size
/// tables on a software rasteriser (the full ones take ~40 s on WARP; the `warp` baselines
/// are for these).
#[must_use]
pub fn tables(dev: &GpuDevice) -> AtmosphereSettings {
    if dev.facts.is_software() {
        AtmosphereSettings::fast()
    } else {
        AtmosphereSettings::default()
    }
}

/// The atmosphere's table precomputation, median of `runs` builds with `settings`: the
/// dispatches' own GPU time from their timestamps (`render.atmosphere.precompute`). Not the
/// wall clock around the blocking build, which also counts any other process's GPU work the
/// driver runs first (72 and 100 ms against the 40.7 ms wall-clock baseline beside another
/// lane's `cargo nextest`, WP-19). The CPU time before the submit and the wall time are
/// printed, not gated: they are driver work of a size that differs by device. A device
/// without timestamps leaves the row unmeasured, which the coverage check fails.
pub fn measure_atmosphere(
    dev: &GpuDevice,
    settings: &AtmosphereSettings,
    runs: usize,
    out: &mut BTreeMap<String, f64>,
) -> Result<(), String> {
    let params = forge_sky::AtmosphereBody::earth()
        .derive()
        .map_err(|e| format!("earth-like air: {e}"))?;
    let mut gpu = Vec::new();
    let mut cpu = Vec::new();
    let mut wall = Vec::new();
    for _ in 0..runs {
        let r = GpuAtmosphere::new(dev, &params, settings)
            .map_err(|e| format!("atmosphere tables: {e}"))?
            .report();
        gpu.extend(r.gpu_ms);
        cpu.push(r.record_ms);
        wall.push(r.precompute_ms);
    }
    println!(
        "atmosphere precompute ({} orders): GPU {gpu:.3?} ms, CPU before submit {cpu:.3?} ms, \
         wall {wall:.3?} ms",
        settings.orders
    );
    if gpu.len() == runs {
        out.insert("render.atmosphere.precompute".into(), median(gpu));
    }
    Ok(())
}

/// The 2D perf frame's output size.
pub const FRAME2D_W: u32 = 1920;
pub const FRAME2D_H: u32 = 1080;

/// The 2D render path's perf frame (`forge_2d::scenes::perf_scene`, 1920x1080): per-pass GPU
/// medians (`render2d.frame.gpu.<pass>` and their sum), the last mile's CPU median, draw
/// calls and graph rebuilds over the steady frames, rendered with `faults`.
pub fn measure_2d(
    dev: &GpuDevice,
    faults: forge_2d::Faults2d,
    out: &mut BTreeMap<String, f64>,
    calib: f64,
) -> Result<(), String> {
    use forge_2d::render::{RenderOptions2d, Renderer2d};
    let opts = RenderOptions2d {
        timing: true,
        faults,
        ..RenderOptions2d::new(FRAME2D_W, FRAME2D_H)
    };
    let mut r = Renderer2d::new(dev, opts).map_err(|e| format!("2d renderer: {e}"))?;
    let (t, atlas) =
        forge_2d::scenes::upload(&mut r, dev).map_err(|e| format!("2d textures: {e}"))?;
    let frame =
        forge_2d::scenes::perf_scene(&t, &atlas).map_err(|e| format!("2d perf scene: {e}"))?;
    let target = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("perf gate 2d target"),
        size: wgpu::Extent3d {
            width: FRAME2D_W,
            height: FRAME2D_H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let mut gpu: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut prep = Vec::new();
    let mut draw_calls = 0.0;
    let mut rebuilds = 0.0;
    for n in 0..40 {
        let rep = r
            .render(dev, &frame, &target)
            .map_err(|e| format!("2d render: {e}"))?;
        if n < 10 {
            continue;
        }
        prep.push(rep.prepare_ns as f64 / 1e6);
        draw_calls = f64::from(rep.prepare.draw_calls);
        rebuilds += f64::from(u8::from(rep.graph_rebuilt));
        for p in &rep.passes {
            if let (Some(ns), Some(pass)) = (p.gpu_ns, p.name.strip_prefix("2d.")) {
                gpu.entry(format!("render2d.frame.gpu.{pass}"))
                    .or_default()
                    .push(ns / 1e6);
            }
        }
    }
    let errors = dev.take_uncaptured_errors();
    if !errors.is_empty() {
        return Err(format!("the 2D perf frame raised GPU errors: {errors:?}"));
    }
    let mut total = 0.0;
    for (k, v) in gpu {
        let m = median(v);
        total += m;
        out.insert(k, m);
    }
    if total > 0.0 {
        out.insert("render2d.frame.gpu.total".into(), total);
    }
    out.insert("render2d.frame.prepare_cpu".into(), median(prep) / calib);
    out.insert("render2d.frame.draw_calls".into(), draw_calls);
    out.insert("render2d.frame.steady_graph_rebuilds".into(), rebuilds);
    Ok(())
}

/// The 2D CPU rows: one solver step with 2,000 bodies on the level's terrain (`phys2d.step`,
/// median of 60 steps) and the 2D cold start (`2d.cold_start`, median of 7).
pub fn measure_2d_cpu(
    faults: forge_2d::Faults2d,
    out: &mut BTreeMap<String, f64>,
    calib: f64,
) -> Result<(), String> {
    let mut w = forge_2d::scenes::physics_pile(2000).map_err(|e| format!("physics pile: {e}"))?;
    w.faults = faults;
    let mut steps = Vec::new();
    for _ in 0..60 {
        let t0 = Instant::now();
        w.step();
        steps.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    std::hint::black_box(w.state_bits());
    let mut cold = Vec::new();
    for _ in 0..7 {
        let t0 = Instant::now();
        std::hint::black_box(
            forge_2d::scenes::cold_start(forge_2d::sprite::TextureId(0))
                .map_err(|e| format!("cold start: {e}"))?,
        );
        cold.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "2d cpu: step {:.3} ms, cold start {:.3} ms (calibration {calib:.3} ms)",
        median(steps.clone()),
        median(cold.clone())
    );
    out.insert("phys2d.step".into(), median(steps) / calib);
    out.insert("2d.cold_start".into(), median(cold) / calib);
    Ok(())
}

/// The 3D physics rows (WP-60, ADR 0066): one fixed step of the budget scene — 2,000
/// dynamic bodies (spheres, boxes, capsules) piled on a floor, `forge_phys::scenes::pile`,
/// after 60 settling steps — on every first-party backend (`phys.step.avian3d`,
/// `phys.step.rapier3d`), median of 60 steps. 2,000 bodies at 60 Hz is M7-9's budget case.
pub fn measure_phys_cpu(
    faults: forge_phys::PhysFaults,
    out: &mut BTreeMap<String, f64>,
    calib: f64,
) -> Result<(), String> {
    for b in forge_phys::FIRST_PARTY {
        let mut w =
            forge_phys::scenes::pile(b, 2000, 60).map_err(|e| format!("physics pile: {e}"))?;
        w.faults = faults;
        let mut steps = Vec::new();
        for _ in 0..60 {
            let t0 = Instant::now();
            w.step().map_err(|e| format!("physics step: {e}"))?;
            steps.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        std::hint::black_box(w.state_hash().map_err(|e| e.to_string())?);
        println!(
            "3d physics ({b}): 2,000 bodies, step {:.3} ms, {} contacts (calibration {calib:.3} ms)",
            median(steps.clone()),
            w.contact_count()
        );
        out.insert(format!("phys.step.{b}"), median(steps) / calib);
    }
    Ok(())
}

/// The measured table, as the gate prints it and writes it to
/// `$CARGO_TARGET_DIR/<file>` (the source for a deliberate baseline update; baselines are
/// never raised to make a gate pass, W5).
pub fn report(
    measured: &BTreeMap<String, f64>,
    class: Option<&str>,
    calib: f64,
    label: &str,
    file: &str,
) -> String {
    let mut s = format!(
        "perf gate measurements ({label}), device class {class:?}, calibration {calib:.3} ms\n"
    );
    for (k, v) in measured {
        s += &format!("{k:40} {v:.4}\n");
    }
    let path = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| repo_root().join("target"), std::path::PathBuf::from)
        .join(file);
    let _ = std::fs::write(&path, &s);
    s
}

/// Every row of `b` at its budget on `class` (the baseline, the ratio, the maximum): what a
/// clean run exactly at the allowance's floor would measure.
#[must_use]
pub fn at_baseline(b: &Budgets, class: &str) -> BTreeMap<String, f64> {
    b.rows
        .iter()
        .map(|r| {
            let v = match r.kind {
                Kind::GpuMs => r.baseline.get(class).copied().unwrap_or(0.0),
                Kind::CpuRatio => r.ratio.unwrap_or(0.0),
                Kind::Counter => r.max.unwrap_or(0.0),
            };
            (r.name.clone(), v)
        })
        .collect()
}
