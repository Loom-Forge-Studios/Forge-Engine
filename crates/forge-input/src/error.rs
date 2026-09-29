//! `InputError` — forge-input's errors (`INPUT-*` codes, `docs/error-codes.md`).

use std::fmt;

/// Every way an input-runtime operation can fail.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputError {
    /// `INPUT-0001`: a binding-overrides file does not parse, or is a newer version.
    BadOverrides(String),
    /// `INPUT-0002`: a binding-overrides file could not be read or written.
    Io { path: String, detail: String },
    /// `INPUT-0003`: a profile name is not 1-64 of a-z, A-Z, 0-9, `_`, `-`.
    BadProfile(String),
    /// `INPUT-0004`: no such local player.
    NoPlayer(u8),
    /// `INPUT-0005`: a device backend could not start (the reason follows).
    Backend(String),
}

impl InputError {
    /// The stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BadOverrides(_) => "INPUT-0001",
            Self::Io { .. } => "INPUT-0002",
            Self::BadProfile(_) => "INPUT-0003",
            Self::NoPlayer(_) => "INPUT-0004",
            Self::Backend(_) => "INPUT-0005",
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<InputError> {
        vec![
            Self::BadOverrides("x".into()),
            Self::Io {
                path: "p".into(),
                detail: "d".into(),
            },
            Self::BadProfile("x".into()),
            Self::NoPlayer(9),
            Self::Backend("x".into()),
        ]
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.code();
        match self {
            Self::BadOverrides(e) => write!(f, "{c}: the binding overrides do not parse: {e}"),
            Self::Io { path, detail } => write!(f, "{c}: cannot read or write {path}: {detail}"),
            Self::BadProfile(p) => write!(
                f,
                "{c}: {p:?} is not a profile name (1-64 of a-z, A-Z, 0-9, _, -)"
            ),
            Self::NoPlayer(p) => write!(f, "{c}: there is no local player {p}"),
            Self::Backend(e) => write!(f, "{c}: an input backend could not start: {e}"),
        }
    }
}

impl std::error::Error for InputError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code is allocated in `docs/error-codes.md` under forge-input (W8), and its
    /// `Display` carries it (greppable).
    #[test]
    fn every_code_is_registered_and_shown() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/error-codes.md");
        let md = std::fs::read_to_string(path).unwrap_or_default();
        for e in InputError::all_variants_for_tests() {
            let row = format!("| {} | forge-input |", e.code());
            assert!(md.contains(&row), "{} is not registered", e.code());
            assert!(e.to_string().starts_with(e.code()));
        }
    }
}
