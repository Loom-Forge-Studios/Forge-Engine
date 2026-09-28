//! Named budgets (Ch.29): an allowance per dotted name, measured by the zone or counter of
//! the same name, grouped by subsystem (the name's first segment).

use std::collections::BTreeMap;

use crate::TraceError;

/// A named budget and its latest measurement, with the statistics a report prints (the
/// profiler panel shows its name, value, allowance and unit).
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub name: String,
    /// Latest measured value (ms, or a count).
    pub value: f64,
    /// The allowance.
    pub budget: f64,
    /// `"ms"` or `""` (a counter).
    pub unit: &'static str,
    /// Measurements that were over the allowance since the budget was declared (or cleared).
    pub over_frames: u64,
    /// Measurements taken.
    pub samples: u64,
    /// The largest measurement.
    pub peak: f64,
}

impl Budget {
    #[must_use]
    pub fn new(name: &str, budget: f64, unit: &'static str) -> Self {
        Self {
            name: name.to_owned(),
            value: 0.0,
            budget,
            unit,
            over_frames: 0,
            samples: 0,
            peak: 0.0,
        }
    }

    /// Over its allowance now: the panel shows it red.
    #[must_use]
    pub fn over(&self) -> bool {
        self.value > self.budget
    }

    /// The subsystem: the name's first dotted segment (`render` for `render.frame.gpu.sky`).
    #[must_use]
    pub fn subsystem(&self) -> &str {
        self.name.split('.').next().unwrap_or("")
    }

    /// Record a measurement; returns whether it was over.
    pub(crate) fn observe(&mut self, v: f64) -> bool {
        self.value = v;
        self.samples += 1;
        if v > self.peak {
            self.peak = v;
        }
        let over = v > self.budget;
        if over {
            self.over_frames += 1;
        }
        over
    }

    pub(crate) fn reset(&mut self) {
        self.value = 0.0;
        self.over_frames = 0;
        self.samples = 0;
        self.peak = 0.0;
    }
}

/// `(name, allowance, unit)` for every absolute row of a `tests/perf/budgets.ron` text (see
/// [`crate::Tracer::declare_budgets_ron`]).
pub fn parse_budgets_ron(
    text: &str,
    class: &str,
) -> Result<Vec<(String, f64, &'static str)>, TraceError> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        kind: Kind,
        #[serde(default)]
        baseline: BTreeMap<String, f64>,
        #[serde(default)]
        max: Option<f64>,
    }
    #[derive(serde::Deserialize)]
    enum Kind {
        GpuMs,
        CpuRatio,
        Counter,
    }
    #[derive(serde::Deserialize)]
    struct File {
        band: f64,
        /// The GPU rows' relative band (WP-18); files before it used `band` for both.
        #[serde(default)]
        gpu_band: Option<f64>,
        #[serde(default)]
        gpu_slack_ms: BTreeMap<String, f64>,
        rows: Vec<Row>,
    }
    let opts =
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
    let f: File = opts
        .from_str(text)
        .map_err(|e| TraceError::BadBudgets(e.to_string()))?;
    let slack = f.gpu_slack_ms.get(class).copied().unwrap_or(0.0);
    let gpu_band = f.gpu_band.unwrap_or(f.band);
    let mut out = Vec::new();
    for r in f.rows {
        match r.kind {
            Kind::GpuMs => {
                if let Some(b) = r.baseline.get(class) {
                    out.push((r.name, (b * (1.0 + gpu_band)).max(b + slack), "ms"));
                }
            }
            Kind::Counter => {
                if let Some(m) = r.max {
                    out.push((r.name, m, ""));
                }
            }
            Kind::CpuRatio => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_budget_file_declares_gpu_and_counter_rows() {
        let text = include_str!("../../../tests/perf/budgets.ron");
        let rows = parse_budgets_ron(text, "rtx3080").unwrap();
        assert!(
            rows.iter()
                .any(|(n, b, u)| n == "render2d.frame.gpu.total" && *b > 0.0 && *u == "ms"),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|(n, _, u)| n == "render2d.frame.draw_calls" && u.is_empty())
        );
        assert!(parse_budgets_ron("(", "x").is_err());
    }

    #[test]
    fn subsystem_and_over() {
        let mut b = Budget::new("render.frame.gpu.sky", 1.0, "ms");
        assert_eq!(b.subsystem(), "render");
        assert!(!b.observe(0.5));
        assert!(b.observe(1.5));
        assert!(b.over());
        assert_eq!((b.over_frames, b.samples, b.peak), (1, 2, 1.5));
    }
}
