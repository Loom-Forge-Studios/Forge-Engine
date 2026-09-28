//! `SimError` — the play core's errors (`SIM-*` codes, `docs/error-codes.md`).

use std::fmt;

use forge_cmd::EntityKey;
use forge_core::{CodedError, ErrorCode, error_code};

/// Every way a play-core operation can fail.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SimError {
    /// `SIM-0001`: an entity of the edit world cannot be simulated as it stands (its
    /// transform frame is not a frame id).
    BadEditWorld { entity: EntityKey, detail: String },
    /// `SIM-0002`: an input names an entity the simulation does not have.
    UnknownEntity(EntityKey),
    /// `SIM-0003`: an input carries a NaN or infinite value.
    NonFinite(EntityKey),
    /// `SIM-0004`: an input arrived with no simulation running (press Play first).
    NotPlaying,
    /// `SIM-0005`: a replay file does not parse (line, reason) or is another format version.
    BadRecording { line: usize, detail: String },
    /// `SIM-0006`: a replay was asked to run against an edit world other than the one it
    /// was recorded from (the scene differs: its hash names both).
    SceneMismatch { recorded: String, actual: String },
    /// `SIM-0007`: a replay diverged from its recording: the first event that differs,
    /// with the step it happened at.
    Diverged {
        step: u64,
        expected: String,
        got: String,
    },
    /// `SIM-0008`: the scheduler refused a tick (the `CORE-*` error follows).
    Core(String),
    /// `SIM-0009`: a replay file could not be read or written.
    Io { path: String, detail: String },
}

impl SimError {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::BadEditWorld { .. } => error_code!("SIM-0001"),
            Self::UnknownEntity(_) => error_code!("SIM-0002"),
            Self::NonFinite(_) => error_code!("SIM-0003"),
            Self::NotPlaying => error_code!("SIM-0004"),
            Self::BadRecording { .. } => error_code!("SIM-0005"),
            Self::SceneMismatch { .. } => error_code!("SIM-0006"),
            Self::Diverged { .. } => error_code!("SIM-0007"),
            Self::Core(_) => error_code!("SIM-0008"),
            Self::Io { .. } => error_code!("SIM-0009"),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<SimError> {
        let k = EntityKey(1);
        vec![
            Self::BadEditWorld {
                entity: k,
                detail: "d".into(),
            },
            Self::UnknownEntity(k),
            Self::NonFinite(k),
            Self::NotPlaying,
            Self::BadRecording {
                line: 1,
                detail: "d".into(),
            },
            Self::SceneMismatch {
                recorded: "a".into(),
                actual: "b".into(),
            },
            Self::Diverged {
                step: 1,
                expected: "a".into(),
                got: "b".into(),
            },
            Self::Core("CORE-0001".into()),
            Self::Io {
                path: "p".into(),
                detail: "d".into(),
            },
        ]
    }
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.code();
        match self {
            Self::BadEditWorld { entity, detail } => {
                write!(f, "{c}: entity {} cannot be simulated: {detail}", entity.0)
            }
            Self::UnknownEntity(e) => {
                write!(f, "{c}: entity {} is not in the simulation", e.0)
            }
            Self::NonFinite(e) => write!(
                f,
                "{c}: an input for entity {} is not finite (NaN or infinite)",
                e.0
            ),
            Self::NotPlaying => write!(f, "{c}: no simulation is running (press Play first)"),
            Self::BadRecording { line, detail } => {
                write!(f, "{c}: replay file line {line}: {detail}")
            }
            Self::SceneMismatch { recorded, actual } => write!(
                f,
                "{c}: the scene differs from the one recorded (edit hash {actual}, recorded {recorded})"
            ),
            Self::Diverged {
                step,
                expected,
                got,
            } => write!(
                f,
                "{c}: replay diverged at step {step}: recorded {expected}, replayed {got}"
            ),
            Self::Core(e) => write!(f, "{c}: the scheduler refused a tick: {e}"),
            Self::Io { path, detail } => write!(f, "{c}: {path}: {detail}"),
        }
    }
}

impl std::error::Error for SimError {}

impl CodedError for SimError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
