//! `ReflectError` — forge-reflect's error enum (Ch.1.2). Codes `REFLECT-0001..0008`,
//! allocated in `docs/error-codes.md`.

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

/// Everything registration, validation and pin connection can refuse.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReflectError {
    /// `REFLECT-0001`: a `units = "..."` annotation does not parse.
    InvalidUnit {
        /// The text as written.
        units: String,
        /// Why it does not parse.
        why: String,
    },
    /// `REFLECT-0002`: an attribute that needs a particular kind of value is on a pin of
    /// another kind (`units` on a `bool`, `range` on a `String`, `entity` on an `f64`).
    MetaOnWrongKind {
        /// The item's path.
        item: String,
        /// The pin or field.
        pin: String,
        /// The attribute (`units`, `range`, `entity`, ...).
        attr: &'static str,
        /// The pin's kind.
        kind: &'static str,
    },
    /// `REFLECT-0003`: `min > max`, a non-positive `step`, or a non-finite bound.
    BadRange {
        /// The item's path.
        item: String,
        /// The pin or field.
        pin: String,
        /// What is wrong.
        why: &'static str,
    },
    /// `REFLECT-0004`: two different items claim the same path.
    DuplicateItem {
        /// The contested path.
        path: String,
    },
    /// `REFLECT-0005`: no item is registered under this path.
    UnknownItem {
        /// The path asked for.
        path: String,
    },
    /// `REFLECT-0006`: an item's four outputs disagree (Ch.6 `test_four_outputs_agree`).
    Drift {
        /// The item's path.
        item: String,
        /// Every disagreement found, one line each.
        problems: Vec<String>,
    },
    /// `REFLECT-0007`: two pins of different types cannot be connected.
    IncompatibleTypes {
        /// Source pin type.
        from: String,
        /// Destination pin type.
        to: String,
    },
    /// `REFLECT-0008`: two pins whose units have different dimensions cannot be connected
    /// (`N·s` into `m/s`).
    IncompatibleUnits {
        /// Source unit.
        from: String,
        /// Destination unit.
        to: String,
    },
}

impl ReflectError {
    /// The variant's stable code.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidUnit { .. } => error_code!("REFLECT-0001"),
            Self::MetaOnWrongKind { .. } => error_code!("REFLECT-0002"),
            Self::BadRange { .. } => error_code!("REFLECT-0003"),
            Self::DuplicateItem { .. } => error_code!("REFLECT-0004"),
            Self::UnknownItem { .. } => error_code!("REFLECT-0005"),
            Self::Drift { .. } => error_code!("REFLECT-0006"),
            Self::IncompatibleTypes { .. } => error_code!("REFLECT-0007"),
            Self::IncompatibleUnits { .. } => error_code!("REFLECT-0008"),
        }
    }
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::InvalidUnit { units, why } => write!(f, "unit `{units}` does not parse: {why}"),
            Self::MetaOnWrongKind {
                item,
                pin,
                attr,
                kind,
            } => write!(f, "`{attr}` on {item}.{pin}, whose kind is {kind}"),
            Self::BadRange { item, pin, why } => write!(f, "{item}.{pin}: {why}"),
            Self::DuplicateItem { path } => {
                write!(f, "two different #[forge_api] items claim `{path}`")
            }
            Self::UnknownItem { path } => write!(f, "no #[forge_api] item `{path}`"),
            Self::Drift { item, problems } => {
                write!(
                    f,
                    "the four outputs of `{item}` disagree: {}",
                    problems.join("; ")
                )
            }
            Self::IncompatibleTypes { from, to } => {
                write!(f, "cannot connect a `{from}` pin to a `{to}` pin")
            }
            Self::IncompatibleUnits { from, to } => write!(
                f,
                "cannot connect a pin in `{from}` to a pin in `{to}`: different dimensions"
            ),
        }
    }
}

impl std::error::Error for ReflectError {}

impl CodedError for ReflectError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}

#[cfg(test)]
impl ReflectError {
    /// One of every variant, for the registration test.
    pub(crate) fn all_variants_for_tests() -> Vec<Self> {
        let s = String::new;
        vec![
            Self::InvalidUnit {
                units: s(),
                why: s(),
            },
            Self::MetaOnWrongKind {
                item: s(),
                pin: s(),
                attr: "",
                kind: "",
            },
            Self::BadRange {
                item: s(),
                pin: s(),
                why: "",
            },
            Self::DuplicateItem { path: s() },
            Self::UnknownItem { path: s() },
            Self::Drift {
                item: s(),
                problems: vec![],
            },
            Self::IncompatibleTypes { from: s(), to: s() },
            Self::IncompatibleUnits { from: s(), to: s() },
        ]
    }
}
