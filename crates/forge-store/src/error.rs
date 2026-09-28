//! `StoreError` — the `STORE-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

/// Every way a store operation can fail. Converts into `forge_core::Error` with `?`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreError {
    /// `STORE-0001`: a path is not a portable project path (see `StorePath`).
    BadPath {
        /// The path.
        path: String,
        /// Why.
        why: &'static str,
    },
    /// `STORE-0002`: no file, blob or revision has this name.
    NotFound(String),
    /// `STORE-0003`: the backend's I/O failed.
    Io {
        /// What was being done.
        op: &'static str,
        /// On what.
        path: String,
        /// The OS error.
        why: String,
    },
    /// `STORE-0004`: stored data does not match its content address, or a record does not
    /// parse. The store refuses to hand back bytes it cannot vouch for.
    Corrupt {
        /// What.
        what: String,
        /// Why.
        why: String,
    },
    /// `STORE-0005`: the path is locked by someone else (Ch.33.3).
    Locked {
        /// The path.
        path: String,
        /// Who holds the lock.
        owner: String,
    },
    /// `STORE-0006`: only the lock's owner can release it.
    NotLockOwner {
        /// The path.
        path: String,
        /// Who holds it.
        owner: String,
        /// Who tried.
        by: String,
    },
    /// `STORE-0007`: a path differs from an existing one only by letter case. It would be
    /// the same file on Windows and a different one on Linux, so it is refused everywhere.
    CaseCollision {
        /// The new path.
        path: String,
        /// The existing path it collides with.
        existing: String,
    },
    /// `STORE-0008`: a store URL names a backend no plugin registered.
    UnknownBackend(String),
    /// `STORE-0009`: a revision id is not 64 hex digits.
    BadRevId(String),
    /// `STORE-0010`: a remote could not be reached, or answered what the protocol does not
    /// allow.
    Remote {
        /// The remote (its URL, credentials never included).
        remote: String,
        /// Why.
        why: String,
    },
    /// `STORE-0011`: the remote moved since it was read (someone pushed meanwhile): nothing
    /// was changed on it. Pull, then push again.
    RemoteMoved {
        /// The remote.
        remote: String,
    },
    /// `STORE-0012`: the remote refused this identity: sign in (GitHub: the device flow), or
    /// the stored token has expired or lacks access to the repository.
    Unauthorized {
        /// The remote.
        remote: String,
        /// What it said.
        why: String,
    },
}

impl StoreError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::BadPath { .. } => error_code!("STORE-0001"),
            Self::NotFound(_) => error_code!("STORE-0002"),
            Self::Io { .. } => error_code!("STORE-0003"),
            Self::Corrupt { .. } => error_code!("STORE-0004"),
            Self::Locked { .. } => error_code!("STORE-0005"),
            Self::NotLockOwner { .. } => error_code!("STORE-0006"),
            Self::CaseCollision { .. } => error_code!("STORE-0007"),
            Self::UnknownBackend(_) => error_code!("STORE-0008"),
            Self::BadRevId(_) => error_code!("STORE-0009"),
            Self::Remote { .. } => error_code!("STORE-0010"),
            Self::RemoteMoved { .. } => error_code!("STORE-0011"),
            Self::Unauthorized { .. } => error_code!("STORE-0012"),
        }
    }

    pub(crate) fn io(op: &'static str, path: impl fmt::Display, e: &std::io::Error) -> Self {
        Self::Io {
            op,
            path: path.to_string(),
            why: e.to_string(),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<StoreError> {
        let s = || "x".to_string();
        vec![
            Self::BadPath {
                path: s(),
                why: "y",
            },
            Self::NotFound(s()),
            Self::Io {
                op: "read",
                path: s(),
                why: s(),
            },
            Self::Corrupt {
                what: s(),
                why: s(),
            },
            Self::Locked {
                path: s(),
                owner: s(),
            },
            Self::NotLockOwner {
                path: s(),
                owner: s(),
                by: s(),
            },
            Self::CaseCollision {
                path: s(),
                existing: s(),
            },
            Self::UnknownBackend(s()),
            Self::BadRevId(s()),
            Self::Remote {
                remote: s(),
                why: s(),
            },
            Self::RemoteMoved { remote: s() },
            Self::Unauthorized {
                remote: s(),
                why: s(),
            },
        ]
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::BadPath { path, why } => write!(f, "{path:?} is not a project path: {why}"),
            Self::NotFound(what) => write!(f, "{what} does not exist"),
            Self::Io { op, path, why } => write!(f, "could not {op} {path}: {why}"),
            Self::Corrupt { what, why } => write!(f, "{what} is corrupt: {why}"),
            Self::Locked { path, owner } => write!(f, "{path} is locked by {owner}"),
            Self::NotLockOwner { path, owner, by } => {
                write!(f, "{path} is locked by {owner}; {by} cannot release it")
            }
            Self::CaseCollision { path, existing } => write!(
                f,
                "{path} differs from existing {existing} only by letter case (the same file on Windows, a different one on Linux)"
            ),
            Self::UnknownBackend(s) => write!(f, "no store backend is registered for {s:?}"),
            Self::BadRevId(s) => write!(f, "{s:?} is not a revision id (64 hex digits)"),
            Self::Remote { remote, why } => write!(f, "the remote {remote} failed: {why}"),
            Self::RemoteMoved { remote } => write!(
                f,
                "the remote {remote} moved since it was read (someone pushed meanwhile); nothing was changed on it: pull, then push again"
            ),
            Self::Unauthorized { remote, why } => write!(
                f,
                "the remote {remote} refused access: {why} (sign in again, or check the token can reach this repository)"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl CodedError for StoreError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
