//! `forge-cli` — the headless `forge` binary (Ch.30, Ch.34.4).
//!
//! The `forge` binary serves the editor core headlessly (`forge --headless`), checks WASM
//! plugins (`forge plugin check`) and runs the other headless entry points below.

#![forbid(unsafe_code)]

pub mod app;
pub mod plugin;

use core::fmt;

/// Errors from the `forge` binary. Codes are allocated in `docs/error-codes.md` (Ch.1.2).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CliError {
    /// `CLI-0001`: the command line is not understood (unknown command or flag, a bad value).
    Usage(String),
    /// `CLI-0002`: an engine step failed; carries the engine's own error (code first).
    Step {
        /// Which step.
        step: &'static str,
        /// The engine error, as it displays.
        detail: String,
    },
    /// `CLI-0003`: a self-check failed — the engine ran but produced a wrong result
    /// (an undo that did not restore, a reload that differs, backends that disagree).
    Check {
        /// Which check.
        check: &'static str,
        /// What was observed.
        detail: String,
    },
}

impl CliError {
    /// The stable error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "CLI-0001",
            Self::Step { .. } => "CLI-0002",
            Self::Check { .. } => "CLI-0003",
        }
    }

    /// A failed engine step (`CLI-0002`), carrying the engine's error as it displays.
    pub fn step(step: &'static str, e: impl fmt::Display) -> Self {
        Self::Step {
            step,
            detail: e.to_string(),
        }
    }

    /// A failed self-check (`CLI-0003`).
    pub fn check(check: &'static str, detail: impl Into<String>) -> Self {
        Self::Check {
            check,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) => write!(f, "{}: {m}", self.code()),
            Self::Step { step, detail } => {
                write!(f, "{}: step `{step}` failed: {detail}", self.code())
            }
            Self::Check { check, detail } => {
                write!(f, "{}: self-check `{check}` failed: {detail}", self.code())
            }
        }
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_lead_the_message() {
        for e in [
            CliError::Usage("x".into()),
            CliError::step("seed", "y"),
            CliError::check("undo", "z"),
        ] {
            assert!(e.to_string().starts_with(e.code()), "{e}");
        }
    }
}
