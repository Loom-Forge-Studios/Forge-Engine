//! `forge-project` errors. Every variant carries a stable code allocated in
//! `docs/error-codes.md` (Ch.1.2, W8).

use std::fmt;

use forge_store::StoreError;

/// Why a project lifecycle operation could not be done. The editor shows the message with
/// its code; nothing is dropped silently.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectError {
    /// `PROJECT-0001`: the operation needs an open project and none is open.
    NotOpen,
    /// `PROJECT-0002`: a new project was asked for where a Forge project already is.
    Exists(String),
    /// `PROJECT-0003`: the location holds no Forge project (no `forge-project.ron`).
    NotAProject(String),
    /// `PROJECT-0004`: the project's files do not read (bad RON, a newer format, a value
    /// the project refuses).
    BadFiles(String),
    /// `PROJECT-0005`: push or pull with no remote linked.
    NoRemote,
    /// `PROJECT-0006`: the remote's kind is not supported (SSH Git URLs: use HTTPS) or its
    /// backend is not built yet (S3, SQL: `UNBUILT`).
    UnsupportedRemote {
        /// The URL.
        url: String,
        /// Why, and what works today.
        why: String,
    },
    /// `PROJECT-0007`: the two histories do not line up: a push to a remote that has
    /// revisions this project lacks, or a pull into a project with revisions the remote
    /// lacks. Merging them is the conflicts flow (Ch.37 §37.4), not a fast-forward.
    Diverged(String),
    /// `PROJECT-0008`: unsaved changes would be overwritten (pull, open over a changed
    /// project): save first.
    Unsaved,
    /// `PROJECT-0009`: the project store failed (its `STORE-*` code follows).
    Store(StoreError),
    /// `PROJECT-0010`: building or exporting failed.
    Build(String),
    /// `PROJECT-0011`: signing in to a Git host failed, was declined, expired, or is not
    /// available (no OAuth app configured).
    SignIn(String),
    /// `PROJECT-0012`: another project transfer (a push, pull, clone, sign-in or repository
    /// creation) is still running; the status names it. Try again when it finishes.
    Busy(String),
    /// `PROJECT-0013`: a transfer stopped before it finished (it panicked, or its thread
    /// could not start); nothing after the step it reached was done.
    Interrupted(String),
    /// `PROJECT-0014`: the team role (or the licence) of whoever asked does not allow it
    /// (Ch.37 §37.6; E-57: a lapsed Team licence refuses only the collaboration commands).
    NotPermitted(String),
    /// `PROJECT-0015`: a team operation failed: no such team, member or invite, a wrong or
    /// already used join code, an invite for another account, or it would leave the team
    /// without an Owner.
    Team(String),
    /// `PROJECT-0016`: the team baseline moved since this sandbox's base: pull (Live does it
    /// on its own), then publish.
    BaselineMoved(String),
    /// `PROJECT-0017`: the subtree is claimed by another teammate (Ch.37 §37.4): it is
    /// read-only for everyone else until they release it.
    Claimed(String),
    /// `PROJECT-0018`: a pull stopped at conflicts nobody has resolved yet (Ch.37 §37.4): the
    /// Conflicts panel shows each one with both sides.
    Unresolved(usize),
}

impl ProjectError {
    /// The stable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotOpen => "PROJECT-0001",
            Self::Exists(_) => "PROJECT-0002",
            Self::NotAProject(_) => "PROJECT-0003",
            Self::BadFiles(_) => "PROJECT-0004",
            Self::NoRemote => "PROJECT-0005",
            Self::UnsupportedRemote { .. } => "PROJECT-0006",
            Self::Diverged(_) => "PROJECT-0007",
            Self::Unsaved => "PROJECT-0008",
            Self::Store(_) => "PROJECT-0009",
            Self::Build(_) => "PROJECT-0010",
            Self::SignIn(_) => "PROJECT-0011",
            Self::Busy(_) => "PROJECT-0012",
            Self::Interrupted(_) => "PROJECT-0013",
            Self::NotPermitted(_) => "PROJECT-0014",
            Self::Team(_) => "PROJECT-0015",
            Self::BaselineMoved(_) => "PROJECT-0016",
            Self::Claimed(_) => "PROJECT-0017",
            Self::Unresolved(_) => "PROJECT-0018",
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<ProjectError> {
        vec![
            Self::NotOpen,
            Self::Exists("x".into()),
            Self::NotAProject("x".into()),
            Self::BadFiles("x".into()),
            Self::NoRemote,
            Self::UnsupportedRemote {
                url: "x".into(),
                why: "y".into(),
            },
            Self::Diverged("x".into()),
            Self::Unsaved,
            Self::Store(StoreError::NotFound("x".into())),
            Self::Build("x".into()),
            Self::SignIn("x".into()),
            Self::Busy("x".into()),
            Self::Interrupted("x".into()),
            Self::NotPermitted("x".into()),
            Self::Team("x".into()),
            Self::BaselineMoved("x".into()),
            Self::Claimed("x".into()),
            Self::Unresolved(1),
        ]
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::NotOpen => write!(f, "no project is open"),
            Self::Exists(at) => write!(
                f,
                "there is already a Forge project at {at}; open it, or pick another folder"
            ),
            Self::NotAProject(at) => write!(
                f,
                "{at} holds no Forge project (no forge-project.ron); create one there instead"
            ),
            Self::BadFiles(why) => write!(f, "the project's files do not read: {why}"),
            Self::NoRemote => write!(
                f,
                "no remote is linked: link one in Revision history or the Project settings"
            ),
            Self::UnsupportedRemote { url, why } => write!(f, "{url}: {why}"),
            Self::Diverged(why) => write!(f, "{why}"),
            Self::Unsaved => write!(
                f,
                "there are unsaved changes that this would overwrite: save first"
            ),
            Self::Store(e) => write!(f, "the project store failed: {e}"),
            Self::Build(why) => write!(f, "the build failed: {why}"),
            Self::SignIn(why) => write!(f, "signing in failed: {why}"),
            Self::Busy(what) => write!(f, "{what} is still running: try again when it finishes"),
            Self::Interrupted(why) => write!(f, "the transfer stopped before it finished: {why}"),
            Self::NotPermitted(why) => write!(f, "not permitted: {why}"),
            Self::Team(why) | Self::BaselineMoved(why) | Self::Claimed(why) => {
                write!(f, "{why}")
            }
            Self::Unresolved(n) => write!(
                f,
                "{n} conflict(s) are not resolved yet: choose a side for each in the Conflicts panel"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<StoreError> for ProjectError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}
