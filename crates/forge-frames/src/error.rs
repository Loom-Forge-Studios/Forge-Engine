//! The crate's error type.

use core::fmt;

use crate::FrameId;

/// Errors from resolving positions between frames. Every variant carries a stable code
/// allocated in `docs/error-codes.md` (Ch.1.2), returned by [`FrameError::code`] and printed
/// first by `Display`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum FrameError {
    /// `FRAMES-0001`: the frame is not one the resolver knows.
    UnknownFrame(FrameId),
    /// `FRAMES-0002`: no path connects the two frames.
    Disconnected { from: FrameId, to: FrameId },
    /// An error a [`crate::FrameResolver`] with frames beyond the world frame defines. It
    /// carries its own code, allocated in `docs/error-codes.md` like every other; the
    /// [`crate::WorldFrame`] resolver never returns one.
    Extension(ExtensionError),
}

/// A resolver-defined error: its stable code and its message (without the code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionError {
    /// The stable code (`docs/error-codes.md`).
    pub code: &'static str,
    /// What went wrong, for a person.
    pub message: String,
}

impl FrameError {
    /// The stable error code (`docs/error-codes.md`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownFrame(_) => "FRAMES-0001",
            Self::Disconnected { .. } => "FRAMES-0002",
            Self::Extension(e) => e.code,
        }
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::UnknownFrame(id) => write!(f, "frame {} is not known to the resolver", id.0),
            Self::Disconnected { from, to } => {
                write!(f, "no path connects frames {} and {}", from.0, to.0)
            }
            Self::Extension(e) => f.write_str(&e.message),
        }
    }
}

impl std::error::Error for FrameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_and_lead_the_message() {
        let all = [
            FrameError::UnknownFrame(FrameId(1)),
            FrameError::Disconnected {
                from: FrameId(1),
                to: FrameId(2),
            },
            FrameError::Extension(ExtensionError {
                code: "TEST-0001",
                message: "a resolver's own error".into(),
            }),
        ];
        let mut codes: Vec<_> = all.iter().map(FrameError::code).collect();
        for e in &all {
            assert!(e.to_string().starts_with(e.code()), "{e}");
        }
        assert_eq!(
            all[2].to_string(),
            "TEST-0001: a resolver's own error",
            "an extension error prints its own code and message"
        );
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }
}
