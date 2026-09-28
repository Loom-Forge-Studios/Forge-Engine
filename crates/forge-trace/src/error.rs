//! `TraceError` — forge-trace's errors (`TRACE-*` codes, `docs/error-codes.md`). forge-trace
//! depends on no engine crate; forge-core maps it into the engine error (`CodedError`).

use std::fmt;

/// Every way a forge-trace operation can fail.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TraceError {
    /// `TRACE-0001`: a trace file could not be written.
    Io {
        /// The file.
        path: String,
        /// The OS error.
        detail: String,
    },
    /// `TRACE-0002`: a budgets file (`tests/perf/budgets.ron` format) does not parse.
    BadBudgets(String),
    /// `TRACE-0003`: the Tracy sink was asked for in a build without the `tracy` feature.
    TracyNotBuilt,
}

impl TraceError {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "TRACE-0001",
            Self::BadBudgets(_) => "TRACE-0002",
            Self::TracyNotBuilt => "TRACE-0003",
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<TraceError> {
        vec![
            Self::Io {
                path: "p".into(),
                detail: "d".into(),
            },
            Self::BadBudgets("x".into()),
            Self::TracyNotBuilt,
        ]
    }
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.code();
        match self {
            Self::Io { path, detail } => write!(f, "{c}: cannot write trace {path}: {detail}"),
            Self::BadBudgets(e) => write!(f, "{c}: budgets file does not parse: {e}"),
            Self::TracyNotBuilt => write!(
                f,
                "{c}: this build has no Tracy sink (rebuild with the `tracy` feature of forge-trace)"
            ),
        }
    }
}

impl std::error::Error for TraceError {}
